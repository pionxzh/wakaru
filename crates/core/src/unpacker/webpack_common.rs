//! Container syntax shared by webpack 4 and webpack 5 output.
//!
//! Both major versions render module tables through the same template
//! (`Template.getModulesArrayBounds`): a sparse array when module ids are
//! dense numerics, wrapped in `Array(minId).concat([...])` when the smallest
//! id is non-zero. The matchers live here so neither version's unpacker owns
//! the other's syntax.

use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, CallExpr,
    Callee, CondExpr, Decl, Expr, Ident, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleItem, Pat, SimpleAssignTarget, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::emit_esm::{dedup_filename, FilenameDedupStyle};
use crate::analysis::binding_uses::{BindingId, BindingUseIndex};
use crate::rules::rename_utils::{collect_module_names, rename_bindings_in_module, BindingRename};
use crate::utils::paren::strip_parens;

const JAVASCRIPT_LIKE_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FactoryNormalizationError {
    /// The factory loader parameter is written, but its loader/local lifetime
    /// boundary cannot be proved. This is the only normalization failure that
    /// webpack extraction may isolate to one opaque factory.
    LoaderParameterReuse,
    /// Any other normalization failure remains container-fatal.
    Fatal,
}

/// Derive a truthful JavaScript output filename from a webpack module id.
///
/// String ids preserve their sanitized resource path. Loader queries and URL
/// fragments are identities inside webpack's table, not filesystem path
/// components, so they are removed here; [`unique_webpack_module_filenames`]
/// then keeps multiple virtual modules for the same resource distinct.
/// Non-JavaScript source extensions are retained as provenance and followed by
/// `.js` (`style.less` -> `style.less.js`). Numeric ids keep webpack's
/// established `module-<id>.js` naming discipline.
pub(super) fn webpack_module_filename(module_id: &str) -> String {
    if module_id.parse::<i64>().is_ok() {
        return format!("module-{module_id}.js");
    }

    let resource_end = module_id
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '?' | '#').then_some(index))
        .unwrap_or(module_id.len());
    let resource = &module_id[..resource_end];
    if resource.is_empty() {
        return "unknown.js".to_string();
    }
    let path_like = resource.contains(['/', '\\', '.']) || resource_end != module_id.len();
    if !path_like {
        return format!("module-{resource}.js");
    }

    let mut filename = super::sanitize_relative_path(resource, "unknown.js");
    let javascript_like = std::path::Path::new(&filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            JAVASCRIPT_LIKE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        });
    if !javascript_like {
        filename.push_str(".js");
    }
    filename
}

/// Allocate webpack module filenames in table order before synthesizing any
/// consumer edge. Doing this at the id->filename boundary is required for
/// collisions such as `a.less` / `a.less.js` and queried virtual modules: a
/// later old-filename rewrite cannot recover which original id owned an edge
/// once two provisional filenames are identical.
pub(super) fn unique_webpack_module_filenames<'a>(
    module_ids: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    module_ids
        .into_iter()
        .map(|module_id| {
            let filename = webpack_module_filename(module_id);
            dedup_filename(
                &filename,
                &mut seen,
                FilenameDedupStyle::PathAware {
                    fallback_stem: "module",
                },
            )
        })
        .collect()
}

/// Whether the stripped webpack factory loader parameter is later reused as a
/// writable local. Such a parameter cannot be renamed wholesale to the free
/// `require` spelling: only its proven module-loader calls have that role.
pub(super) fn runtime_parameter_is_written(
    module: &Module,
    parameter: &Atom,
    unresolved_mark: Mark,
) -> bool {
    let id = (
        parameter.clone(),
        SyntaxContext::empty().apply_mark(unresolved_mark),
    );
    BindingUseIndex::collect(module).has_direct_write(&id)
}

/// Recover a webpack loader parameter's second lifetime as a real module-local
/// binding after mapped loader calls and runtime helpers have been separated.
///
/// Minifiers commonly emit `value = load(id); load = /re/; ...`. The factory
/// parameter is local in the original program, but the wrapper-free module
/// previously printed the second lifetime as an assignment to free `require`.
/// This routine lifts the first proven write into a `var` initializer at the
/// same top-level evaluation point, then scope-aware-renames every later use.
/// Nested mapped `require()` calls in that initializer are captured in their
/// own declarations so `UnEsm` can recover static imports. If the first use is
/// not a supported, unconditional write prefix, the caller must fail closed.
pub(super) fn localize_reused_runtime_parameter(
    module: &mut Module,
    parameter: &Atom,
    unresolved_mark: Mark,
) -> bool {
    // A literal `require` parameter cannot share a printed spelling with the
    // canonical loader calls while retaining a distinct local binding.
    if parameter.as_ref() == "require" {
        return false;
    }

    let target = (
        parameter.clone(),
        SyntaxContext::empty().apply_mark(unresolved_mark),
    );
    let mut candidate = module.clone();
    let mut used_names = collect_module_names(&candidate);
    let local_name = fresh_runtime_value_name(parameter, &mut used_names);
    let local = Ident::new(local_name.clone(), DUMMY_SP, target.1);

    let mut rebuilt = Vec::with_capacity(candidate.body.len() + 2);
    let mut items = std::mem::take(&mut candidate.body).into_iter();
    let mut localized = false;
    while let Some(mut item) = items.next() {
        if let Some(replacement) = lift_first_runtime_parameter_write(
            &mut item,
            &target,
            &local,
            unresolved_mark,
            &mut used_names,
        ) {
            rebuilt.extend(replacement);
            rebuilt.extend(items);
            localized = true;
            break;
        }
        if item_contains_binding(&item, &target) {
            return false;
        }
        rebuilt.push(item);
    }
    if !localized {
        return false;
    }
    candidate.body = rebuilt;

    rename_bindings_in_module(
        &mut candidate,
        &[BindingRename {
            old: target.clone(),
            new: local_name,
        }],
    );
    let remaining = BindingUseIndex::collect(&candidate);
    if remaining.use_count(&target) != 0 || remaining.has_declaration(&target) {
        return false;
    }

    *module = candidate;
    true
}

fn fresh_runtime_value_name(parameter: &Atom, used_names: &mut HashSet<Atom>) -> Atom {
    let base = Atom::from(format!("_{parameter}"));
    if used_names.insert(base.clone()) {
        return base;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = Atom::from(format!("_{parameter}{suffix}"));
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn lift_first_runtime_parameter_write(
    item: &mut ModuleItem,
    target: &BindingId,
    local: &Ident,
    unresolved_mark: Mark,
    used_names: &mut HashSet<Atom>,
) -> Option<Vec<ModuleItem>> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
            for index in 0..var.decls.len() {
                let Some(init) = var.decls[index].init.as_mut() else {
                    continue;
                };
                let Some(initializer) = take_leading_parameter_assignment(init, target) else {
                    continue;
                };
                let mut replacement = Vec::new();
                if index > 0 {
                    let mut before = (**var).clone();
                    before.decls = var.decls[..index].to_vec();
                    replacement.push(var_decl_item(before));
                }
                replacement.extend(runtime_value_initializer_items(
                    local.clone(),
                    initializer,
                    unresolved_mark,
                    used_names,
                ));
                let mut after = (**var).clone();
                after.decls = var.decls[index..].to_vec();
                replacement.push(var_decl_item(after));
                return Some(replacement);
            }
            None
        }
        ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
            let initializer = take_leading_parameter_assignment(&mut expr_stmt.expr, target)?;
            let mut replacement = runtime_value_initializer_items(
                local.clone(),
                initializer,
                unresolved_mark,
                used_names,
            );
            replacement.push(item.clone());
            Some(replacement)
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) => {
            let initializer = take_leading_parameter_assignment(&mut export.expr, target)?;
            let mut replacement = runtime_value_initializer_items(
                local.clone(),
                initializer,
                unresolved_mark,
                used_names,
            );
            replacement.push(item.clone());
            Some(replacement)
        }
        _ => None,
    }
}

fn take_leading_parameter_assignment(
    expr: &mut Box<Expr>,
    target: &BindingId,
) -> Option<Box<Expr>> {
    take_leading_parameter_assignment_expr(expr.as_mut(), target)
}

fn take_leading_parameter_assignment_expr(
    expr: &mut Expr,
    target: &BindingId,
) -> Option<Box<Expr>> {
    match expr {
        Expr::Assign(assign)
            if assign.op == AssignOp::Assign
                && simple_assignment_ident(&assign.left)
                    .is_some_and(|ident| ident.sym == target.0 && ident.ctxt == target.1) =>
        {
            let mut safety = ReadBeforeWrite::new(target);
            // A function value is created now but its body observes the local
            // only after this assignment completes. Nested functions inside
            // any other initializer are treated conservatively because they
            // may be invoked before the outer write finishes.
            if !matches!(strip_parens(&assign.right), Expr::Fn(_) | Expr::Arrow(_)) {
                assign.right.visit_with(&mut safety);
            }
            if safety.read_before_write {
                return None;
            }
            let initializer = assign.right.clone();
            *expr = Expr::Ident(Ident::new(target.0.clone(), assign.span, target.1));
            Some(initializer)
        }
        Expr::Assign(assign) => match &mut assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                take_leading_parameter_assignment_expr(member.obj.as_mut(), target)
            }
            _ => None,
        },
        Expr::Paren(paren) => take_leading_parameter_assignment_expr(paren.expr.as_mut(), target),
        Expr::Seq(sequence) => sequence
            .exprs
            .first_mut()
            .and_then(|first| take_leading_parameter_assignment_expr(first.as_mut(), target)),
        Expr::Member(member) => take_leading_parameter_assignment_expr(member.obj.as_mut(), target),
        Expr::Call(call) => match &mut call.callee {
            Callee::Expr(callee) => take_leading_parameter_assignment_expr(callee.as_mut(), target),
            _ => None,
        },
        Expr::Bin(binary) => take_leading_parameter_assignment_expr(binary.left.as_mut(), target),
        Expr::Cond(cond) => take_leading_parameter_assignment_expr(cond.test.as_mut(), target),
        Expr::Unary(unary) => take_leading_parameter_assignment_expr(unary.arg.as_mut(), target),
        _ => None,
    }
}

fn simple_assignment_ident(target: &AssignTarget) -> Option<&Ident> {
    match target {
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => Some(&binding.id),
        _ => None,
    }
}

fn runtime_value_initializer_items(
    local: Ident,
    mut initializer: Box<Expr>,
    unresolved_mark: Mark,
    used_names: &mut HashSet<Atom>,
) -> Vec<ModuleItem> {
    let mut captures = Vec::new();
    if !is_static_require_call(&initializer, unresolved_mark) {
        if let Some(require) = leading_static_require_mut(&mut initializer, unresolved_mark) {
            let local_name = fresh_dependency_name(used_names);
            let local = Ident::new(local_name, DUMMY_SP, SyntaxContext::empty());
            let require = Box::new(std::mem::replace(require, Expr::Ident(local.clone())));
            captures.push(var_binding_item(local, require));
        }
    }
    captures.push(var_binding_item(local, initializer));
    captures
}

fn var_binding_item(local: Ident, initializer: Box<Expr>) -> ModuleItem {
    var_decl_item(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent::from(local)),
            init: Some(initializer),
            definite: false,
        }],
    })
}

fn var_decl_item(decl: VarDecl) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(decl))))
}

fn is_static_require_call(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    matches!(callee.as_ref(), Expr::Ident(ident)
        if ident.sym.as_ref() == "require" && ident.ctxt.outer() == unresolved_mark)
        && call.args.len() == 1
        && call.args[0].spread.is_none()
        && matches!(call.args[0].expr.as_ref(), Expr::Lit(Lit::Str(_)))
}

/// Find a static loader call only along the expression's guaranteed first
/// evaluation path. Hoisting a later call across an earlier side effect would
/// change program order, so unsupported shapes deliberately return `None`.
fn leading_static_require_mut(expr: &mut Expr, unresolved_mark: Mark) -> Option<&mut Expr> {
    if is_static_require_call(expr, unresolved_mark) {
        return Some(expr);
    }

    match expr {
        Expr::Paren(paren) => leading_static_require_mut(&mut paren.expr, unresolved_mark),
        Expr::Seq(sequence) => sequence
            .exprs
            .first_mut()
            .and_then(|first| leading_static_require_mut(first, unresolved_mark)),
        Expr::Member(member) => leading_static_require_mut(&mut member.obj, unresolved_mark),
        Expr::Call(call) => match &mut call.callee {
            Callee::Expr(callee) => leading_static_require_mut(callee, unresolved_mark),
            _ => None,
        },
        Expr::Bin(binary) => leading_static_require_mut(&mut binary.left, unresolved_mark),
        Expr::Cond(cond) => leading_static_require_mut(&mut cond.test, unresolved_mark),
        Expr::Unary(unary) => leading_static_require_mut(&mut unary.arg, unresolved_mark),
        _ => None,
    }
}

fn fresh_dependency_name(used_names: &mut HashSet<Atom>) -> Atom {
    let base = Atom::from("_dependency");
    if used_names.insert(base.clone()) {
        return base;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = Atom::from(format!("_dependency{suffix}"));
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

struct BindingFinder<'a> {
    target: &'a BindingId,
    found: bool,
}

impl Visit for BindingFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.target.0 && ident.ctxt == self.target.1 {
            self.found = true;
        }
    }
}

fn item_contains_binding(item: &ModuleItem, target: &BindingId) -> bool {
    let mut finder = BindingFinder {
        target,
        found: false,
    };
    item.visit_with(&mut finder);
    finder.found
}

/// Tracks the original parameter's value while evaluating the initializer of
/// its first lifted assignment. Reads are safe only after an inner simple
/// assignment has established the local value. A root function initializer is
/// skipped by the caller; nested closures are conservatively rejected because
/// the surrounding expression may invoke them immediately.
struct ReadBeforeWrite<'a> {
    target: &'a BindingId,
    initialized: bool,
    read_before_write: bool,
}

impl<'a> ReadBeforeWrite<'a> {
    fn new(target: &'a BindingId) -> Self {
        Self {
            target,
            initialized: false,
            read_before_write: false,
        }
    }
}

impl Visit for ReadBeforeWrite<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.target.0 && ident.ctxt == self.target.1 && !self.initialized {
            self.read_before_write = true;
        }
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let Some(ident) = simple_assignment_ident(&assign.left) {
            if ident.sym == self.target.0 && ident.ctxt == self.target.1 {
                if assign.op != AssignOp::Assign && !self.initialized {
                    self.read_before_write = true;
                }
                assign.right.visit_with(self);
                self.initialized = true;
                return;
            }
        }
        assign.visit_children_with(self);
    }

    fn visit_cond_expr(&mut self, cond: &CondExpr) {
        cond.test.visit_with(self);
        let after_test = self.initialized;
        let mut consequent = Self {
            target: self.target,
            initialized: after_test,
            read_before_write: self.read_before_write,
        };
        cond.cons.visit_with(&mut consequent);
        let mut alternate = Self {
            target: self.target,
            initialized: after_test,
            read_before_write: self.read_before_write,
        };
        cond.alt.visit_with(&mut alternate);
        self.read_before_write = consequent.read_before_write || alternate.read_before_write;
        self.initialized = consequent.initialized && alternate.initialized;
    }

    fn visit_bin_expr(&mut self, binary: &BinExpr) {
        binary.left.visit_with(self);
        if matches!(
            binary.op,
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing
        ) {
            let after_left = self.initialized;
            let mut right = Self {
                target: self.target,
                initialized: after_left,
                read_before_write: self.read_before_write,
            };
            binary.right.visit_with(&mut right);
            self.read_before_write |= right.read_before_write;
            self.initialized = after_left && right.initialized;
            return;
        }
        binary.right.visit_with(self);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let mut finder = BindingFinder {
            target: self.target,
            found: false,
        };
        function.visit_with(&mut finder);
        self.read_before_write |= finder.found && !self.initialized;
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let mut finder = BindingFinder {
            target: self.target,
            found: false,
        };
        arrow.visit_with(&mut finder);
        self.read_before_write |= finder.found && !self.initialized;
    }

    fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
        let mut finder = BindingFinder {
            target: self.target,
            found: false,
        };
        class.visit_with(&mut finder);
        self.read_before_write |= finder.found && !self.initialized;
    }
}

/// Match `Array(<n>).concat([...])` — webpack's sparse-array header when the
/// smallest module id is non-zero. Returns the array literal and the id
/// offset `n`.
pub(super) fn split_array_concat(call: &CallExpr) -> Option<(&ArrayLit, usize)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee) else {
        return None;
    };
    let MemberProp::Ident(concat_ident) = prop else {
        return None;
    };
    if concat_ident.sym.as_ref() != "concat" {
        return None;
    }
    let Expr::Call(array_call) = strip_parens(obj) else {
        return None;
    };
    let Callee::Expr(array_callee) = &array_call.callee else {
        return None;
    };
    let Expr::Ident(array_ident) = strip_parens(array_callee) else {
        return None;
    };
    if array_ident.sym.as_ref() != "Array" {
        return None;
    }
    if array_call.args.len() != 1 || array_call.args[0].spread.is_some() {
        return None;
    }
    let id_offset = numeric_id_from_expr(&array_call.args[0].expr)?;
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    let Expr::Array(array) = strip_parens(&call.args[0].expr) else {
        return None;
    };
    Some((array, id_offset))
}

/// A non-negative integer literal module id. Bounded to `i32::MAX`: real
/// module ids sit far below it, and an unbounded literal
/// (`Array(1e100).concat([...])`) would saturate the float→usize cast and
/// overflow the `id_offset + index` arithmetic downstream — including on
/// wasm32, where `usize` is 32 bits and even `u32::MAX + index` wraps.
pub(super) fn numeric_id_from_expr(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parens(expr) else {
        return None;
    };
    let value = number.value;
    if value < 0.0 || value.fract() != 0.0 || value > f64::from(i32::MAX) {
        return None;
    }
    Some(value as usize)
}
