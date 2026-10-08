//! Authored top-level code around a bundle's runtime statement: a raw banner
//! or footer, or scripts concatenated with the bundle. It runs before or after
//! the bundle, so unpackers keep it verbatim in entry.js, in source order
//! around the startup, instead of writing it to no output file.

use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{Expr, ExprStmt, Ident, Lit, Module, ModuleItem, Stmt};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::rules::rename_utils::{rename_bindings_in_module, BindingRename};

/// The input's top-level items before and after the bundle statement.
#[derive(Clone, Copy, Default)]
pub(super) struct SurroundingItems<'a> {
    before: &'a [ModuleItem],
    after: &'a [ModuleItem],
}

impl<'a> SurroundingItems<'a> {
    /// The items around `body[idx]`.
    pub(super) fn around(body: &'a [ModuleItem], idx: usize) -> Self {
        Self {
            before: &body[..idx],
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
    /// module's `require`), and the declaration would capture them.
    pub(super) fn collect(items: SurroundingItems<'_>, entry_free_names: &[&str]) -> Option<Self> {
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

        let mut probe = Module {
            span: DUMMY_SP,
            body: items.before.iter().chain(items.after).cloned().collect(),
            shebang: None,
        };
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        probe.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let mut names = SurroundingNameCollector {
            unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
            top_level_ctxt: SyntaxContext::empty().apply_mark(top_level_mark),
            names: HashSet::default(),
            top_level_names: HashSet::default(),
            all_names: HashSet::default(),
        };
        probe.visit_with(&mut names);
        if entry_free_names
            .iter()
            .any(|name| names.top_level_names.contains(&Atom::from(*name)))
        {
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

/// Splits the identifiers of code around the bundle by resolution.
struct SurroundingNameCollector {
    unresolved_ctxt: SyntaxContext,
    top_level_ctxt: SyntaxContext,
    /// Free names and top-level bindings.
    names: HashSet<Atom>,
    top_level_names: HashSet<Atom>,
    all_names: HashSet<Atom>,
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
