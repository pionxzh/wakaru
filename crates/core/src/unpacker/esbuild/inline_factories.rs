//! Name a lazy factory that a minifier inlined into its only call.
//!
//! rolldown's minifier inlines a CommonJS factory used once into the call
//! that runs it: `var …, c = l(o(((e, t) => { … }))())`. The detector finds
//! factories only as `var x = helper(factory)` declarators, so this pass gives
//! such a factory a declarator of its own, just before the one that calls it:
//! `var …, require_c = o((e, t) => { … }), c = l(require_c())`.
//!
//! The move changes no behavior: `helper(factory)` only builds a closure, the
//! factory runs when that closure is called, and the call stays where it was.
//! It is limited to code that runs exactly once at top level (a top-level
//! `var` initializer or expression statement, with no function, class, or
//! accessor between the call and the module), to a factory that is a function
//! or arrow expression, and to a helper declared by one top-level `var`
//! declarator before the call and never written, so the closure is built from
//! the same helper value. A helper imported from another input qualifies too
//! when that input proves it (see `imported_helpers`): imports are bound
//! before any code runs, the importer cannot write them, and the exporter's
//! proof covers writes in its own module.

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, CallExpr, Callee, Class, Decl, Expr, ExprOrSpread, FnDecl, Function, GetterProp,
    Ident, Module, ModuleDecl, ModuleItem, Pat, SetterProp, Stmt, VarDecl, VarDeclKind,
    VarDeclarator,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::unpacker::BindingId;
use crate::utils::paren::{strip_parens, strip_parens_owned};

use super::bindings::collect_write_bindings;

/// Whether a top-level evaluation position calls a helper's closure on the
/// spot (`helper(factory)()`). A cheap precheck before cloning the module.
pub(super) fn has_inlined_factory_calls(module: &Module, helper_syms: &HashSet<Atom>) -> bool {
    !inlined_factory_helpers(module, helper_syms, true).is_empty()
}

/// The helpers among `helper_syms` whose closure a top-level evaluation
/// position calls on the spot; only the first one found with `first_only`.
pub(super) fn inlined_factory_helpers(
    module: &Module,
    helper_syms: &HashSet<Atom>,
    first_only: bool,
) -> HashSet<Atom> {
    let mut finder = InlineCallFinder {
        helper_syms,
        first_only,
        found: HashSet::default(),
    };
    if helper_syms.is_empty() {
        return finder.found;
    }
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) if var.kind == VarDeclKind::Var => {
                for decl in &var.decls {
                    if let Some(init) = &decl.init {
                        init.visit_with(&mut finder);
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Expr(stmt)) => stmt.expr.visit_with(&mut finder),
            _ => {}
        }
        if finder.done() {
            break;
        }
    }
    finder.found
}

/// Give each inlined factory call its own `var` declarator. Returns whether
/// the module changed.
///
/// A helper in `imported_helpers` is an import binding another input proved
/// to be a lazy helper that its module never writes; the importer cannot
/// write an import binding, and imports are bound before any code runs, so
/// such a helper needs no declaration check here.
pub(super) fn name_inlined_factories(
    module: &mut Module,
    helper_syms: &HashSet<Atom>,
    imported_helpers: &HashSet<Atom>,
) -> bool {
    if !has_inlined_factory_calls(module, helper_syms) {
        return false;
    }
    let local_helpers: HashSet<Atom> = helper_syms
        .iter()
        .filter(|sym| !imported_helpers.contains(*sym))
        .cloned()
        .collect();
    let mut helpers: HashMap<Atom, Option<(usize, usize)>> =
        unwritten_helper_positions(module, &local_helpers)
            .into_iter()
            .map(|(sym, position)| (sym, Some(position)))
            .collect();
    helpers.extend(
        imported_helpers
            .iter()
            .filter(|sym| helper_syms.contains(*sym))
            .map(|sym| (sym.clone(), None)),
    );
    if helpers.is_empty() {
        return false;
    }

    let mut names = FreshNames::new(module);
    let mut changed = false;
    let body = std::mem::take(&mut module.body);
    let mut rewritten = Vec::with_capacity(body.len());
    for (item_index, mut item) in body.into_iter().enumerate() {
        match &mut item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) if var.kind == VarDeclKind::Var => {
                let decls = std::mem::take(&mut var.decls);
                let mut out = Vec::with_capacity(decls.len());
                for (decl_index, mut decl) in decls.into_iter().enumerate() {
                    if let Some(init) = decl.init.as_mut() {
                        let hint = match &decl.name {
                            Pat::Ident(binding) => Some(binding.id.sym.clone()),
                            _ => None,
                        };
                        let named = take_inlined_factories(
                            init,
                            &helpers,
                            (item_index, decl_index),
                            hint.as_ref(),
                            &mut names,
                        );
                        changed |= !named.is_empty();
                        out.extend(named);
                    }
                    out.push(decl);
                }
                var.decls = out;
            }
            ModuleItem::Stmt(Stmt::Expr(stmt)) => {
                let named = take_inlined_factories(
                    &mut stmt.expr,
                    &helpers,
                    (item_index, usize::MAX),
                    None,
                    &mut names,
                );
                if !named.is_empty() {
                    changed = true;
                    rewritten.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                        span: DUMMY_SP,
                        ctxt: SyntaxContext::empty(),
                        kind: VarDeclKind::Var,
                        declare: false,
                        decls: named,
                    })))));
                }
            }
            _ => {}
        }
        rewritten.push(item);
    }
    module.body = rewritten;
    changed
}

/// Position `(item, declarator)` of each helper that one top-level `var`
/// declarator declares and nothing writes, proven on a resolved clone.
pub(super) fn unwritten_helper_positions(
    module: &Module,
    helper_syms: &HashSet<Atom>,
) -> HashMap<Atom, (usize, usize)> {
    let mut resolved = module.clone();
    resolved.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));

    let mut declared: HashMap<Atom, (BindingId, (usize, usize))> = HashMap::default();
    let mut repeated: HashSet<Atom> = HashSet::default();
    for (item_index, item) in resolved.body.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        if var.kind != VarDeclKind::Var {
            continue;
        }
        for (decl_index, decl) in var.decls.iter().enumerate() {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            if !helper_syms.contains(&binding.id.sym) || decl.init.is_none() {
                continue;
            }
            let id = (binding.id.sym.clone(), binding.id.ctxt);
            if declared
                .insert(binding.id.sym.clone(), (id, (item_index, decl_index)))
                .is_some()
            {
                repeated.insert(binding.id.sym.clone());
            }
        }
    }
    for sym in &repeated {
        declared.remove(sym);
    }
    if declared.is_empty() {
        return HashMap::default();
    }

    let ids: HashSet<BindingId> = declared.values().map(|(id, _)| id.clone()).collect();
    // Any other declaration of the same binding (a second `var`, a function
    // or class declaration) or any write disqualifies the helper.
    let mut counter = BindingDeclCounter {
        ids: &ids,
        counts: HashMap::default(),
    };
    resolved.visit_with(&mut counter);
    let mut writes = HashSet::default();
    for item in &resolved.body {
        match item {
            ModuleItem::Stmt(stmt) => collect_write_bindings(stmt, &ids, &mut writes),
            // Bundler output declares nothing inside its import and export
            // lists, so nothing in them writes a binding.
            ModuleItem::ModuleDecl(
                ModuleDecl::Import(_) | ModuleDecl::ExportNamed(_) | ModuleDecl::ExportAll(_),
            ) => {}
            // Code inside any other module declaration is not scanned.
            ModuleItem::ModuleDecl(_) => return HashMap::default(),
        }
    }

    declared
        .into_iter()
        .filter(|(_, (id, _))| counter.counts.get(id) == Some(&1) && !writes.contains(id))
        .map(|(sym, (_, position))| (sym, position))
        .collect()
}

/// Replace every inlined `helper(factory)()` in `expr` by `name()` and return
/// the declarators `name = helper(factory)` to place before it.
fn take_inlined_factories(
    expr: &mut Expr,
    helpers: &HashMap<Atom, Option<(usize, usize)>>,
    position: (usize, usize),
    hint: Option<&Atom>,
    names: &mut FreshNames,
) -> Vec<VarDeclarator> {
    let mut rewriter = InlineCallRewriter {
        helpers,
        position,
        hint,
        names,
        named: Vec::new(),
    };
    expr.visit_mut_with(&mut rewriter);
    rewriter.named
}

/// The helper and factory of `helper(factory)()`, when the inner call passes
/// one function or arrow expression (in any number of parentheses).
fn inlined_factory_call(call: &CallExpr) -> Option<(&Ident, &Expr)> {
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Call(inner) = strip_parens(callee) else {
        return None;
    };
    let Callee::Expr(helper) = &inner.callee else {
        return None;
    };
    let Expr::Ident(helper) = strip_parens(helper) else {
        return None;
    };
    let [ExprOrSpread { spread: None, expr }] = inner.args.as_slice() else {
        return None;
    };
    matches!(strip_parens(expr), Expr::Arrow(_) | Expr::Fn(_)).then_some((helper, &**expr))
}

struct InlineCallFinder<'a> {
    helper_syms: &'a HashSet<Atom>,
    first_only: bool,
    found: HashSet<Atom>,
}

impl InlineCallFinder<'_> {
    fn done(&self) -> bool {
        self.first_only && !self.found.is_empty()
    }
}

impl Visit for InlineCallFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.done() {
            return;
        }
        if let Some((helper, _)) = inlined_factory_call(call) {
            if self.helper_syms.contains(&helper.sym) {
                self.found.insert(helper.sym.clone());
                // The rest of the call is the factory, which runs later.
                return;
            }
        }
        call.visit_children_with(self);
    }

    // Code in these runs later, or more than once.
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
    fn visit_getter_prop(&mut self, _: &GetterProp) {}
    fn visit_setter_prop(&mut self, _: &SetterProp) {}
}

struct InlineCallRewriter<'a> {
    helpers: &'a HashMap<Atom, Option<(usize, usize)>>,
    position: (usize, usize),
    hint: Option<&'a Atom>,
    names: &'a mut FreshNames,
    named: Vec<VarDeclarator>,
}

impl VisitMut for InlineCallRewriter<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        let declared_before = inlined_factory_call(call).is_some_and(|(helper, _)| {
            self.helpers
                .get(&helper.sym)
                .is_some_and(|helper_position| {
                    helper_position.is_none_or(|declared| declared < self.position)
                })
        });
        if !declared_before {
            call.visit_mut_children_with(self);
            return;
        }
        let Callee::Expr(callee) = &mut call.callee else {
            unreachable!("checked by inlined_factory_call");
        };
        let span: Span = callee.span();
        let name = self.names.fresh(self.hint);
        let ident = Ident::new(name.clone(), span, SyntaxContext::empty());
        let inner = std::mem::replace(callee, Box::new(Expr::Ident(ident.clone())));
        let Expr::Call(mut inner) = strip_parens_owned(*inner) else {
            unreachable!("checked by inlined_factory_call");
        };
        if let Some(arg) = inner.args.first_mut() {
            let factory = std::mem::replace(&mut *arg.expr, Expr::Invalid(Default::default()));
            *arg.expr = strip_parens_owned(factory);
        }
        self.named.push(VarDeclarator {
            span,
            name: Pat::Ident(Ident::new(name, DUMMY_SP, SyntaxContext::empty()).into()),
            init: Some(Box::new(Expr::Call(inner))),
            definite: false,
        });
    }

    // Code in these runs later, or more than once.
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
    fn visit_mut_class(&mut self, _: &mut Class) {}
    fn visit_mut_getter_prop(&mut self, _: &mut GetterProp) {}
    fn visit_mut_setter_prop(&mut self, _: &mut SetterProp) {}
}

/// Counts the declarations of each tracked binding: `var` declarators,
/// function and class declarations, and any other binding pattern.
struct BindingDeclCounter<'a> {
    ids: &'a HashSet<BindingId>,
    counts: HashMap<BindingId, usize>,
}

impl BindingDeclCounter<'_> {
    fn count(&mut self, ident: &Ident) {
        let id = (ident.sym.clone(), ident.ctxt);
        if self.ids.contains(&id) {
            *self.counts.entry(id).or_default() += 1;
        }
    }
}

impl Visit for BindingDeclCounter<'_> {
    fn visit_binding_ident(&mut self, binding: &swc_core::ecma::ast::BindingIdent) {
        self.count(&binding.id);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.count(&decl.ident);
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &swc_core::ecma::ast::ClassDecl) {
        self.count(&decl.ident);
        decl.class.visit_with(self);
    }
}

/// Names that no identifier in the module uses: `require_<hint>`, or
/// `require_inline` when the call sits outside a declarator.
struct FreshNames {
    used: HashSet<Atom>,
}

impl FreshNames {
    fn new(module: &Module) -> Self {
        struct Atoms(HashSet<Atom>);
        impl Visit for Atoms {
            fn visit_ident(&mut self, ident: &Ident) {
                self.0.insert(ident.sym.clone());
            }
        }
        let mut atoms = Atoms(HashSet::default());
        module.visit_with(&mut atoms);
        Self { used: atoms.0 }
    }

    fn fresh(&mut self, hint: Option<&Atom>) -> Atom {
        let base = match hint {
            Some(hint) => format!("require_{hint}"),
            None => "require_inline".to_string(),
        };
        let mut candidate: Atom = base.clone().into();
        let mut suffix = 2usize;
        while self.used.contains(&candidate) {
            candidate = format!("{base}_{suffix}").into();
            suffix += 1;
        }
        self.used.insert(candidate.clone());
        candidate
    }
}
