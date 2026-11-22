use crate::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::Result;
use sourcemap::SourceMap;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap as SwcSourceMap};
use swc_core::ecma::ast::{
    ImportSpecifier, MemberProp, Module, ModuleDecl, ModuleItem, PropName, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::driver::line_index::LineIndex;
use crate::js_names::is_valid_identifier_name;
use crate::rules::decl_utils::collect_decl_binding_ids;
use crate::rules::rename_utils::{rename_bindings_in_module, BindingId, BindingRename};

/// Parse a source map from raw bytes.
pub fn parse_sourcemap(data: &[u8]) -> Result<SourceMap> {
    SourceMap::from_reader(BufReader::new(data))
        .map_err(|e| anyhow::anyhow!("failed to parse source map: {e}"))
}

/// Extract every original source entry from a source map as `(relative_path, content)` pairs.
///
/// The paths are sanitized (bundler prefixes stripped, path traversal prevented)
/// and resolved relative to `out_dir`. No filesystem writes are performed.
pub fn extract_source_entries(sm: &SourceMap, out_dir: &Path) -> Vec<(PathBuf, String)> {
    let count = sm.get_source_count();
    let mut entries = Vec::new();

    for i in 0..count {
        let Some(source_path) = sm.get_source(i) else {
            continue;
        };
        let Some(content) = sm.get_source_contents(i) else {
            continue;
        };

        let out_path = resolve_source_path(out_dir, source_path);
        entries.push((out_path, content.to_string()));
    }

    entries
}

/// Map a source map entry path to an output path under `out_dir`.
///
/// Source map paths often start with `../` (relative to the bundle's location)
/// or carry webpack's URL schemes: `webpack://` in emitted maps, and
/// `webpack-internal://` in maps of `eval` devtool builds. We strip these and
/// resolve the result under `out_dir`, never escaping it: `..` segments, a
/// root, and a drive are dropped, with `/` and `\` both read as separators
/// (see [`crate::driver::sanitize_relative_path`]).
pub fn resolve_source_path(out_dir: &Path, source: &str) -> PathBuf {
    let stripped = ["webpack-internal://", "webpack://"]
        .into_iter()
        .find_map(|scheme| source.strip_prefix(scheme))
        .unwrap_or(source);
    // Join the cleaned name once rather than pushing segments: on Windows,
    // pushing a segment such as `D:` replaces the whole path.
    let relative = crate::driver::sanitize_relative_path(stripped, "");
    if relative.is_empty() {
        out_dir.to_path_buf()
    } else {
        out_dir.join(relative)
    }
}

/// Apply source-map-driven identifier renaming to `module`.
///
/// Strategy:
/// 1. For every identifier, look up its generated position in the source map,
///    map it to the original source file + position, and extract the identifier
///    at that position from `sourcesContent`.
/// 2. Vote on the original name per binding `(sym, SyntaxContext)`.
/// 3. Apply plurality winners as renames, with the following disambiguation rule:
///    - **Local bindings** (function params, block-scoped locals): all claimants for
///      the same name get the bare name — they live in nested scopes and shadow each
///      other just like in the original source.
///    - **Module-level bindings** (imports, top-level let/const/var/fn): claimants
///      must be unique because the bundler merged all original module scopes into one
///      flat namespace.
pub fn apply_sourcemap_renames(
    module: &mut Module,
    sm: &SourceMap,
    cm: &Lrc<SwcSourceMap>,
    unresolved_mark: Mark,
) {
    let renames = collect_sourcemap_renames(module, sm, cm, unresolved_mark);
    rename_bindings_in_module(module, &renames);
}

fn collect_sourcemap_renames(
    module: &Module,
    sm: &SourceMap,
    cm: &Lrc<SwcSourceMap>,
    unresolved_mark: Mark,
) -> Vec<BindingRename> {
    let module_level_ids = collect_module_level_bindings(module);

    // Index every original source once. A minified vendor file can be one
    // long line that many identifiers map into, so turning a (line, UTF-16
    // column) into a byte offset must not rescan the line on each lookup.
    let source_count = sm.get_source_count();
    let original_sources: Vec<Option<(&str, LineIndex)>> = (0..source_count)
        .map(|i| {
            sm.get_source_contents(i)
                .map(|content| (content, LineIndex::new(content)))
        })
        .collect();

    // Index the generated source once so a BytePos becomes a source map
    // (line, UTF-16 column) with binary searches, not the O(col) scan that
    // cm.lookup_char_pos() performs on a long minified line.
    let src_file = cm.lookup_source_file(module.span.lo);
    let start_pos = src_file.start_pos.0;
    let generated_lines = LineIndex::new(&src_file.src);

    // Phase 1: scan every identifier and vote on its original name.
    let mut voter = NameVoter {
        sm,
        generated_lines: &generated_lines,
        start_pos,
        original_sources: &original_sources,
        unresolved_mark,
        votes: HashMap::default(),
    };
    module.visit_with(&mut voter);

    // Phase 2: group winning names by claimant type.
    let mut claimants: HashMap<Atom, (Vec<BindingId>, Vec<BindingId>)> = HashMap::default();

    for (binding_id, vote_map) in &voter.votes {
        let Some(winner) = plurality_winner(vote_map) else {
            continue;
        };
        if winner == binding_id.0 {
            continue; // already has the right name
        }
        let entry = claimants.entry(winner).or_default();
        if module_level_ids.contains(binding_id) {
            entry.0.push(binding_id.clone());
        } else {
            entry.1.push(binding_id.clone());
        }
    }

    // Phase 3: emit renames.
    let mut renames = Vec::new();

    for (new_name, (module_claimants, local_claimants)) in claimants {
        // Local bindings: all get the bare name — they live in nested scopes.
        for binding in local_claimants {
            renames.push(BindingRename {
                old: binding,
                new: new_name.clone(),
            });
        }

        // Module-level bindings: must be unique in the merged top-level scope.
        if module_claimants.len() == 1 {
            renames.push(BindingRename {
                old: module_claimants.into_iter().next().unwrap(),
                new: new_name,
            });
        } else {
            for (i, binding) in module_claimants.into_iter().enumerate() {
                let actual: Atom = if i == 0 {
                    new_name.clone()
                } else {
                    format!("{}_{}", new_name, i + 1).into()
                };
                if actual != binding.0 {
                    renames.push(BindingRename {
                        old: binding,
                        new: actual,
                    });
                }
            }
        }
    }

    renames
}

fn plurality_winner(votes: &HashMap<Atom, usize>) -> Option<Atom> {
    votes
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(name, _)| name.clone())
}

/// Collect the BindingId of every binding declared at the top level of the module.
fn collect_module_level_bindings(module: &Module) -> HashSet<BindingId> {
    let mut ids = HashSet::default();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for spec in &import.specifiers {
                    let (sym, ctxt) = match spec {
                        ImportSpecifier::Named(n) => (n.local.sym.clone(), n.local.ctxt),
                        ImportSpecifier::Default(d) => (d.local.sym.clone(), d.local.ctxt),
                        ImportSpecifier::Namespace(n) => (n.local.sym.clone(), n.local.ctxt),
                    };
                    ids.insert((sym, ctxt));
                }
            }
            ModuleItem::Stmt(Stmt::Decl(decl))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(swc_core::ecma::ast::ExportDecl {
                decl,
                ..
            })) => {
                collect_decl_binding_ids(decl, &mut ids);
            }
            _ => {}
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// Visitor that tallies source-map name votes per binding
// ---------------------------------------------------------------------------

struct NameVoter<'a> {
    sm: &'a SourceMap,
    /// Line and UTF-16 column lookup for the generated file.
    generated_lines: &'a LineIndex,
    /// `BytePos.0` of the first byte of the generated source file.
    start_pos: u32,
    /// Content and line index of each original source, by source id.
    original_sources: &'a [Option<(&'a str, LineIndex)>],
    /// Identifiers with this mark as their outermost SyntaxContext are global/unresolved
    /// references (e.g. `Object`, `Symbol`).  We never rename these.
    unresolved_mark: Mark,
    votes: HashMap<BindingId, HashMap<Atom, usize>>,
}

impl Visit for NameVoter<'_> {
    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if ident.span.is_dummy() {
            return;
        }

        // Skip global/unresolved references — resolver() stamps them with unresolved_mark.
        // Renaming globals like `Object` or `Symbol` via source-map lookup produces noise.
        if ident.ctxt.outer() == self.unresolved_mark {
            return;
        }

        // Source map columns count UTF-16 code units, not bytes.
        let relative = ident.span.lo.0.saturating_sub(self.start_pos);
        let Some((sm_line, sm_col)) = self.generated_lines.position(relative) else {
            return;
        };

        // Clone early so `self` is no longer borrowed before the mutable vote access.
        let Some(orig_name) = self.lookup_original_name(sm_line, sm_col).map(Atom::from) else {
            return;
        };

        if orig_name.is_empty() || orig_name == ident.sym {
            return;
        }
        if !is_valid_identifier_name(orig_name.as_ref()) {
            return;
        }

        let binding_id: BindingId = (ident.sym.clone(), ident.ctxt);
        let vote_map = self.votes.entry(binding_id).or_default();
        *vote_map.entry(orig_name).or_insert(0) += 1;
    }

    // Property keys are not bindings — skip them.
    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.expr.visit_with(self);
        }
    }
}

impl NameVoter<'_> {
    fn lookup_original_name(&self, gen_line: u32, gen_col: u32) -> Option<&str> {
        let token = self.sm.lookup_token(gen_line, gen_col)?;

        // Try the `names` array first (present in some source maps, empty in esbuild).
        if let Some(name) = token.get_name() {
            if !name.is_empty() {
                return Some(name);
            }
        }

        // Fall back: extract the identifier from the original source content.
        let src_idx = token.get_src_id() as usize;
        let (content, lines) = self.original_sources.get(src_idx)?.as_ref()?;
        let start = lines.offset(content, token.get_src_line(), token.get_src_col())?;
        extract_identifier_at(&content[start as usize..])
    }
}

// ---------------------------------------------------------------------------
// Identifier extraction — zero allocation
// ---------------------------------------------------------------------------

/// Extract the JS identifier at the start of `text`.
fn extract_identifier_at(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    if !is_ident_start(*bytes.first()?) {
        return None;
    }
    let len = bytes.iter().take_while(|&&b| is_ident_continue(b)).count();
    Some(&text[..len])
}

#[inline]
fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

#[inline]
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}
