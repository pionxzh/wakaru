//! Authored top-level code around a bundle's runtime statement: a raw banner
//! or footer, or scripts concatenated with the bundle. It runs before or after
//! the bundle, so unpackers keep it verbatim in entry.js, in source order
//! around the startup, instead of writing it to no output file.

use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{CallExpr, Callee, Expr, ExprStmt, Ident, Lit, Module, ModuleItem, Stmt};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::module_path::relative_import_specifier;
use crate::rules::rename_utils::{rename_bindings_in_module, BindingRename};
use crate::unpacker::{emit_module_with_source_map, spans_byte_ranges, UnpackedModule};

/// The input's top-level items before and after the bundle statement.
#[derive(Clone, Copy, Default)]
pub(super) struct SurroundingItems<'a> {
    before: &'a [ModuleItem],
    bundle: Option<&'a ModuleItem>,
    after: &'a [ModuleItem],
}

impl<'a> SurroundingItems<'a> {
    /// The items around `body[idx]`, the bundle statement.
    pub(super) fn around(body: &'a [ModuleItem], idx: usize) -> Self {
        Self {
            before: &body[..idx],
            bundle: Some(&body[idx]),
            after: &body[idx + 1..],
        }
    }
}

/// Code around the bundle statement, with the names it uses.
#[derive(Default)]
pub(super) struct SurroundingCode {
    before: Vec<ModuleItem>,
    after: Vec<ModuleItem>,
    /// Names the code reads as globals or declares at its top level. Moved
    /// next to them, entry bindings with these names would capture them.
    names: HashSet<Atom>,
    /// Every identifier name in the code, which renamed entry bindings avoid.
    all_names: HashSet<Atom>,
}

impl SurroundingCode {
    /// `None` when the code declares one of `entry_free_names` at its top
    /// level: entry.js gives those free names a meaning of its own (the
    /// module's `require`), and the declaration would capture them. Also
    /// `None` when a factory in `factory_spans` (inside the bundle statement)
    /// uses a name the code declares at its top level; see
    /// [`factories_use_names`].
    pub(super) fn collect(
        items: SurroundingItems<'_>,
        entry_free_names: &[&str],
        factory_spans: &[Span],
    ) -> Option<Self> {
        // Directives and empty statements alone are no reason to write
        // entry.js.
        let is_inert = |item: &ModuleItem| match item {
            ModuleItem::Stmt(Stmt::Empty(_)) => true,
            ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => {
                matches!(&**expr, Expr::Lit(Lit::Str(_)))
            }
            _ => false,
        };
        if items.before.iter().chain(items.after).all(is_inert) {
            return Some(Self::default());
        }

        let names = SurroundingNameCollector::collect(items.before.iter().chain(items.after));
        if entry_free_names
            .iter()
            .any(|name| names.top_level_names.contains(&Atom::from(*name)))
        {
            return None;
        }
        if factories_use_names(
            &names.top_level_names,
            items.bundle.into_iter(),
            factory_spans,
        ) {
            return None;
        }

        Some(Self {
            before: items.before.to_vec(),
            after: items.after.to_vec(),
            names: names.names,
            all_names: names.all_names,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }

    pub(super) fn spans(&self) -> impl Iterator<Item = Span> + '_ {
        self.before.iter().chain(&self.after).map(Spanned::span)
    }

    /// Rename the resolved entry `module`'s top-level bindings whose names the
    /// code uses. They were bundle-scope locals; next to the code, neither
    /// side may capture the other.
    pub(super) fn rename_conflicts(&self, module: &mut Module, top_level_mark: Mark) {
        if self.is_empty() {
            return;
        }
        let top_level_ctxt = SyntaxContext::empty().apply_mark(top_level_mark);
        let mut bindings = ContextSymCollector::new(top_level_ctxt);
        module.visit_with(&mut bindings);
        let mut conflicts: Vec<Atom> = bindings
            .syms
            .into_iter()
            .filter(|sym| self.names.contains(sym))
            .collect();
        if conflicts.is_empty() {
            return;
        }
        conflicts.sort_unstable();
        let mut taken = IdentNameCollector::default();
        module.visit_with(&mut taken);
        let mut taken = taken.0;
        taken.extend(self.all_names.iter().cloned());
        let renames: Vec<BindingRename> = conflicts
            .into_iter()
            .map(|sym| {
                let mut suffix = 1;
                let mut new = Atom::from(format!("{sym}_{suffix}"));
                while taken.contains(&new) {
                    suffix += 1;
                    new = Atom::from(format!("{sym}_{suffix}"));
                }
                taken.insert(new.clone());
                BindingRename {
                    old: (sym, top_level_ctxt),
                    new,
                }
            })
            .collect();
        rename_bindings_in_module(module, &renames);
    }

    /// The entry file `filename` for bundles whose startup only runs entry
    /// modules: the code before the bundle statement, a `require` of each
    /// entry module in startup order, and the code after it.
    pub(super) fn entry_module(
        &self,
        filename: &str,
        entry_filenames: &[&str],
        cm: &Lrc<SourceMap>,
    ) -> Option<UnpackedModule> {
        let mut module = Module {
            span: DUMMY_SP,
            body: entry_filenames
                .iter()
                .map(|entry| {
                    let specifier = relative_import_specifier(filename, entry);
                    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: Box::new(Expr::Call(CallExpr {
                            callee: Callee::Expr(Box::new(Expr::Ident(Ident::new_no_ctxt(
                                "require".into(),
                                DUMMY_SP,
                            )))),
                            args: vec![Expr::from(specifier.as_str()).into()],
                            ..Default::default()
                        })),
                    }))
                })
                .collect(),
            shebang: None,
        };
        self.place_around(&mut module);
        let (code, generated_source_map) = emit_module_with_source_map(&module, cm.clone()).ok()?;
        Some(UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code,
            filename: filename.to_string(),
            source_ranges: spans_byte_ranges(cm, self.spans()),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map,
            verbatim_source_offset: None,
            mapped_in_every_mode: false,
        })
    }

    /// Place the code before and after the entry `module`'s statements.
    pub(super) fn place_around(&self, module: &mut Module) {
        if self.is_empty() {
            return;
        }
        let entry_items = std::mem::take(&mut module.body);
        module.body = self
            .before
            .iter()
            .cloned()
            .chain(entry_items)
            .chain(self.after.iter().cloned())
            .collect();
    }
}

/// Whether a factory uses a top-level declaration of the code beside the
/// bundle statement (`surrounding`), so the split must not happen.
///
/// The input is a classic script: a top-level declaration is a global binding,
/// and a module factory in the bundle can read or write it as a free name (an
/// obfuscator's string-array decoder, declared before the code it rewrites and
/// called in every factory; producer `javascript-obfuscator@5.9.0
/// --string-array true` over a minified bundle). The split moves the
/// declaration to `entry.js` or a prelude file, where it is module-scoped, and
/// the factory's own file would throw a `ReferenceError` on the name.
pub(super) fn factories_use_declarations<'a>(
    surrounding: impl Iterator<Item = &'a ModuleItem>,
    bundle: impl Iterator<Item = &'a ModuleItem> + Clone,
    factory_spans: &[Span],
) -> bool {
    let names = SurroundingNameCollector::collect(surrounding);
    factories_use_names(&names.top_level_names, bundle, factory_spans)
}

/// Whether code inside `factory_spans`, which lie in the `bundle` statements,
/// uses one of `declared` as a free name: free in the bundle statements, so in
/// the input it is the top-level declaration beside them. A bundle statement
/// is an expression statement and declares no top-level binding itself. Names
/// are compared first, so the bundle is cloned and resolved only when a
/// factory mentions a declared name at all.
fn factories_use_names<'a>(
    declared: &HashSet<Atom>,
    bundle: impl Iterator<Item = &'a ModuleItem> + Clone,
    factory_spans: &[Span],
) -> bool {
    if declared.is_empty() || factory_spans.is_empty() {
        return false;
    }
    let mut spans = factory_spans.to_vec();
    spans.sort_unstable_by_key(|span| span.lo);
    let mut mentions = FactoryNameFinder {
        declared,
        spans: &spans,
        ctxt: None,
        found: false,
    };
    for item in bundle.clone() {
        item.visit_with(&mut mentions);
    }
    if !mentions.found {
        return false;
    }

    let mut probe = Module {
        span: DUMMY_SP,
        body: bundle.cloned().collect(),
        shebang: None,
    };
    let unresolved_mark = Mark::new();
    probe.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
    let mut uses = FactoryNameFinder {
        declared,
        spans: &spans,
        ctxt: Some(SyntaxContext::empty().apply_mark(unresolved_mark)),
        found: false,
    };
    probe.visit_with(&mut uses);
    uses.found
}

/// Finds an identifier inside one of `spans` (sorted by start) named in
/// `declared`, and with `ctxt` when it is set.
struct FactoryNameFinder<'a> {
    declared: &'a HashSet<Atom>,
    spans: &'a [Span],
    ctxt: Option<SyntaxContext>,
    found: bool,
}

impl Visit for FactoryNameFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.found
            || !self.declared.contains(&ident.sym)
            || self.ctxt.is_some_and(|ctxt| ident.ctxt != ctxt)
        {
            return;
        }
        let after = self.spans.partition_point(|span| span.lo <= ident.span.lo);
        self.found = after > 0 && ident.span.hi <= self.spans[after - 1].hi;
    }
}

/// Splits the identifiers of code around the bundle by resolution.
struct SurroundingNameCollector {
    unresolved_ctxt: SyntaxContext,
    top_level_ctxt: SyntaxContext,
    /// Free names and top-level bindings.
    names: HashSet<Atom>,
    top_level_names: HashSet<Atom>,
    all_names: HashSet<Atom>,
}

impl SurroundingNameCollector {
    /// Resolve a copy of `items` as one program and split its identifiers.
    fn collect<'a>(items: impl Iterator<Item = &'a ModuleItem>) -> Self {
        let mut probe = Module {
            span: DUMMY_SP,
            body: items.cloned().collect(),
            shebang: None,
        };
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        probe.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let mut names = Self {
            unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
            top_level_ctxt: SyntaxContext::empty().apply_mark(top_level_mark),
            names: HashSet::default(),
            top_level_names: HashSet::default(),
            all_names: HashSet::default(),
        };
        probe.visit_with(&mut names);
        names
    }
}

impl Visit for SurroundingNameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.all_names.insert(ident.sym.clone());
        if ident.ctxt == self.top_level_ctxt {
            self.top_level_names.insert(ident.sym.clone());
            self.names.insert(ident.sym.clone());
        } else if ident.ctxt == self.unresolved_ctxt {
            self.names.insert(ident.sym.clone());
        }
    }
}

/// Symbols of identifiers that carry one syntax context.
pub(super) struct ContextSymCollector {
    ctxt: SyntaxContext,
    pub(super) syms: HashSet<Atom>,
}

impl ContextSymCollector {
    pub(super) fn new(ctxt: SyntaxContext) -> Self {
        Self {
            ctxt,
            syms: HashSet::default(),
        }
    }
}

impl Visit for ContextSymCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.ctxt == self.ctxt {
            self.syms.insert(ident.sym.clone());
        }
    }
}

/// Every identifier name in a node.
#[derive(Default)]
pub(super) struct IdentNameCollector(pub(super) HashSet<Atom>);

impl Visit for IdentNameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.0.insert(ident.sym.clone());
    }
}
