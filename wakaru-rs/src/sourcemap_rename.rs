use std::collections::{HashMap, HashSet};
use std::io::BufReader;

use anyhow::Result;
use sourcemap::SourceMap;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap as SwcSourceMap};
use swc_core::ecma::ast::{
    Decl, ImportSpecifier, MemberProp, Module, ModuleDecl, ModuleItem, Pat, PropName, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

/// Precompute byte offset of each line start for O(log n) (line, col) lookup.
fn compute_line_starts(src: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, &b) in src.as_bytes().iter().enumerate() {
        if b == b'\n' {
            starts.push((i + 1) as u32);
        }
    }
    starts
}

use crate::rules::rename_utils::{BindingId, BindingRename, rename_bindings_in_module};

/// Parse a source map from raw bytes.
pub fn parse_sourcemap(data: &[u8]) -> Result<SourceMap> {
    SourceMap::from_reader(BufReader::new(data))
        .map_err(|e| anyhow::anyhow!("failed to parse source map: {e}"))
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

    // Pre-process source files into per-line slices.
    // Using &str (zero-copy, borrowed from SourceMap) avoids String allocation overhead.
    let source_count = sm.get_source_count();
    let source_lines: Vec<Vec<&str>> = (0..source_count)
        .map(|i| {
            sm.get_source_contents(i)
                .map(|content| content.lines().collect())
                .unwrap_or_default()
        })
        .collect();

    // Precompute line-start byte offsets from the generated source file so we can
    // convert a BytePos to (line, col) with a binary search instead of the O(col)
    // UTF-8 character scan that cm.lookup_char_pos() performs.
    let src_file = cm.lookup_source_file(module.span.lo);
    let start_pos = src_file.start_pos.0;
    let line_starts = compute_line_starts(&src_file.src);

    // Phase 1: scan every identifier and vote on its original name.
    let mut voter = NameVoter {
        sm,
        line_starts: &line_starts,
        start_pos,
        source_lines: &source_lines,
        unresolved_mark,
        votes: HashMap::new(),
    };
    module.visit_with(&mut voter);

    // Phase 2: group winning names by claimant type.
    let mut claimants: HashMap<Atom, (Vec<BindingId>, Vec<BindingId>)> = HashMap::new();

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
    let mut ids = HashSet::new();
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
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(
                swc_core::ecma::ast::ExportDecl { decl, .. },
            )) => {
                collect_decl_binding_ids(decl, &mut ids);
            }
            _ => {}
        }
    }
    ids
}

fn collect_decl_binding_ids(decl: &Decl, ids: &mut HashSet<BindingId>) {
    match decl {
        Decl::Var(var) => {
            for declarator in &var.decls {
                collect_pat_binding_ids(&declarator.name, ids);
            }
        }
        Decl::Fn(f) => {
            ids.insert((f.ident.sym.clone(), f.ident.ctxt));
        }
        Decl::Class(c) => {
            ids.insert((c.ident.sym.clone(), c.ident.ctxt));
        }
        _ => {}
    }
}

fn collect_pat_binding_ids(pat: &Pat, ids: &mut HashSet<BindingId>) {
    match pat {
        Pat::Ident(bi) => {
            ids.insert((bi.id.sym.clone(), bi.id.ctxt));
        }
        Pat::Array(arr) => {
            for elem in arr.elems.iter().flatten() {
                collect_pat_binding_ids(elem, ids);
            }
        }
        Pat::Object(obj) => {
            for prop in &obj.props {
                use swc_core::ecma::ast::ObjectPatProp;
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_binding_ids(&kv.value, ids),
                    ObjectPatProp::Assign(a) => {
                        ids.insert((a.key.id.sym.clone(), a.key.id.ctxt));
                    }
                    ObjectPatProp::Rest(r) => collect_pat_binding_ids(&r.arg, ids),
                }
            }
        }
        Pat::Rest(r) => collect_pat_binding_ids(&r.arg, ids),
        Pat::Assign(a) => collect_pat_binding_ids(&a.left, ids),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Visitor that tallies source-map name votes per binding
// ---------------------------------------------------------------------------

struct NameVoter<'a> {
    sm: &'a SourceMap,
    /// Byte offsets of each line start within the generated file (relative to file start).
    line_starts: &'a [u32],
    /// `BytePos.0` of the first byte of the generated source file.
    start_pos: u32,
    source_lines: &'a Vec<Vec<&'a str>>,
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

        // Convert BytePos to (line, col) via binary search over precomputed line_starts.
        // This is O(log lines) instead of the O(col) UTF-8 scan that lookup_char_pos does.
        let relative = ident.span.lo.0.saturating_sub(self.start_pos);
        let sm_line = self.line_starts.partition_point(|&s| s <= relative) - 1;
        let sm_col = relative - self.line_starts[sm_line];
        let sm_line = sm_line as u32;

        // Clone early so `self` is no longer borrowed before the mutable vote access.
        let Some(orig_name) = self.lookup_original_name(sm_line, sm_col).map(Atom::from) else {
            return;
        };

        if orig_name.is_empty() || orig_name == ident.sym {
            return;
        }
        if !is_valid_js_identifier(orig_name.as_ref()) {
            return;
        }

        let binding_id: BindingId = (ident.sym.clone(), ident.ctxt);
        let vote_map = self.votes.entry(binding_id).or_default();
        *vote_map.entry(orig_name).or_insert(0) += 1;
    }

    // Property keys are not bindings — skip them.
    fn visit_prop_name(&mut self, _: &PropName) {}

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
        let orig_line = token.get_src_line() as usize;
        let orig_col = token.get_src_col() as usize;

        let line = self.source_lines.get(src_idx)?.get(orig_line)?;
        extract_identifier_at(line, orig_col)
    }
}

// ---------------------------------------------------------------------------
// Identifier extraction — ASCII-fast, zero allocation
// ---------------------------------------------------------------------------

/// Extract a JS identifier starting at byte column `col` (works for ASCII source).
/// Source map columns are in UTF-16 code units; for ASCII this equals the byte offset.
fn extract_identifier_at(line: &str, col: usize) -> Option<&str> {
    let bytes = line.as_bytes();
    let start = col;
    if start >= bytes.len() {
        return None;
    }
    if !is_ident_start(bytes[start]) {
        return None;
    }
    let len = bytes[start..]
        .iter()
        .take_while(|&&b| is_ident_continue(b))
        .count();
    // Safety: all bytes are ASCII (checked by is_ident_start/continue), valid UTF-8.
    Some(unsafe { std::str::from_utf8_unchecked(&bytes[start..start + len]) })
}

#[inline]
fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

#[inline]
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn is_valid_js_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = name.as_bytes();
    if !is_ident_start(bytes[0]) {
        return false;
    }
    if !bytes[1..].iter().all(|&b| is_ident_continue(b)) {
        return false;
    }
    !is_reserved_keyword(name)
}

fn is_reserved_keyword(name: &str) -> bool {
    matches!(
        name,
        "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "let"
            | "new"
            | "null"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "enum"
            | "await"
            | "implements"
            | "interface"
            | "package"
            | "private"
            | "protected"
            | "public"
    )
}
