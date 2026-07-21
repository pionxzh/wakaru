use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{
    sync::Lrc, Globals, Mark, SourceMap, Spanned, SyntaxContext, DUMMY_SP, GLOBALS,
};
use swc_core::ecma::ast::{
    ArrayLit, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmtOrExpr, CallExpr,
    Callee, Expr, ExprStmt, FnExpr, Ident, IdentName, Lit, MemberExpr, MemberProp, Module,
    ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread, SeqExpr, SimpleAssignTarget, Stmt,
    Str, UnaryExpr, UnaryOp, VarDecl, VarDeclarator,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::utils::replace_ident;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::rules::rename_utils::BindingRename;
use crate::unpacker::webpack4::{
    rewrite_require_n_accesses, RequireIdRewriter, RequireStringIdRewriter,
};
use crate::unpacker::{
    deconflict_runtime_binding_renames, emit_module_with_source_map, source_fallback_for_stmts,
    spans_byte_ranges, BundleFormat, DetectedBundle, PreparedModuleAst, UnpackResult,
    UnpackedModule,
};
use crate::utils::paren::strip_parens;
use crate::utils::swc_safety::apply_fixer;

struct Webpack5RuntimeNormalizer {
    require_sym: Atom,
    unresolved_mark: Mark,
}

impl VisitMut for Webpack5RuntimeNormalizer {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(ctxt) = self.try_match_require_member(expr, "g") {
            *expr = Expr::Ident(Ident::new(Atom::from("global"), DUMMY_SP, ctxt));
        } else if let Some(ctxt) = self.try_match_require_member(expr, "amdO") {
            *expr = amd_define_detection_expr(ctxt);
        }
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let mut new_items = Vec::with_capacity(items.len());
        for item in items.drain(..) {
            if let ModuleItem::Stmt(stmt) = item {
                if let Some(replacements) = self.try_convert_stmt(&stmt, true) {
                    new_items.extend(replacements.into_iter().map(ModuleItem::Stmt));
                } else {
                    new_items.push(ModuleItem::Stmt(stmt));
                }
            } else {
                new_items.push(item);
            }
        }
        *items = new_items;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let mut new_stmts = Vec::with_capacity(stmts.len());
        for stmt in stmts.drain(..) {
            if let Some(replacements) = self.try_convert_stmt(&stmt, false) {
                new_stmts.extend(replacements);
            } else {
                new_stmts.push(stmt);
            }
        }
        *stmts = new_stmts;
    }
}

impl Webpack5RuntimeNormalizer {
    fn try_match_require_member(&self, expr: &Expr, expected_prop: &str) -> Option<SyntaxContext> {
        let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
            return None;
        };
        let Expr::Ident(obj_ident) = &**obj else {
            return None;
        };
        if obj_ident.sym != self.require_sym || obj_ident.ctxt.outer() != self.unresolved_mark {
            return None;
        }
        let MemberProp::Ident(prop_name) = prop else {
            return None;
        };
        if prop_name.sym.as_ref() != expected_prop {
            return None;
        }
        Some(obj_ident.ctxt)
    }

    fn try_convert_stmt(
        &self,
        stmt: &Stmt,
        allow_module_decorator_removal: bool,
    ) -> Option<Vec<Stmt>> {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            return None;
        };
        let stripped_expr = strip_parens(expr);
        if allow_module_decorator_removal {
            if self.is_module_decorator_assignment(stripped_expr) {
                return Some(vec![]);
            }
            if let Expr::Seq(seq) = stripped_expr {
                return self.try_remove_module_decorator_from_seq(seq);
            }
        }
        let Expr::Call(call) = stripped_expr else {
            return None;
        };
        let Callee::Expr(callee_expr) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
            return None;
        };
        let Expr::Ident(callee_obj) = &**obj else {
            return None;
        };
        if callee_obj.sym != self.require_sym || callee_obj.ctxt.outer() != self.unresolved_mark {
            return None;
        }
        let MemberProp::Ident(prop_name) = prop else {
            return None;
        };

        match prop_name.sym.as_ref() {
            "r" => None,
            "d" => None,
            _ => None,
        }
    }

    fn try_remove_module_decorator_from_seq(&self, seq: &SeqExpr) -> Option<Vec<Stmt>> {
        let mut changed = false;
        let mut exprs = Vec::with_capacity(seq.exprs.len());
        for expr in &seq.exprs {
            if self.is_module_decorator_assignment(strip_parens(expr)) {
                changed = true;
            } else {
                exprs.push(expr.clone());
            }
        }
        if !changed {
            return None;
        }
        match exprs.len() {
            0 => Some(vec![]),
            1 => Some(vec![Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: exprs.pop().expect("single seq expr"),
            })]),
            _ => Some(vec![Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(Expr::Seq(SeqExpr {
                    span: seq.span,
                    exprs,
                })),
            })]),
        }
    }

    fn is_module_decorator_assignment(&self, expr: &Expr) -> bool {
        let Expr::Assign(AssignExpr {
            op: AssignOp::Assign,
            left,
            right,
            ..
        }) = expr
        else {
            return false;
        };
        let AssignTarget::Simple(SimpleAssignTarget::Ident(left_ident)) = left else {
            return false;
        };
        if left_ident.id.sym.as_ref() != "module" {
            return false;
        }
        let Expr::Call(call) = &**right else {
            return false;
        };
        if call.args.len() != 1 {
            return false;
        }
        let Callee::Expr(callee_expr) = &call.callee else {
            return false;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
            return false;
        };
        let Expr::Ident(callee_obj) = &**obj else {
            return false;
        };
        if callee_obj.sym != self.require_sym || callee_obj.ctxt.outer() != self.unresolved_mark {
            return false;
        }
        let MemberProp::Ident(prop_name) = prop else {
            return false;
        };
        if !matches!(prop_name.sym.as_ref(), "hmd" | "nmd") {
            return false;
        }
        let Expr::Ident(arg_ident) = &*call.args[0].expr else {
            return false;
        };
        arg_ident.sym.as_ref() == "module" && arg_ident.ctxt == left_ident.id.ctxt
    }
}

fn amd_define_detection_expr(ctxt: SyntaxContext) -> Expr {
    Expr::Bin(BinExpr {
        span: DUMMY_SP,
        op: BinaryOp::LogicalAnd,
        left: Box::new(Expr::Bin(BinExpr {
            span: DUMMY_SP,
            op: BinaryOp::EqEqEq,
            left: Box::new(Expr::Unary(UnaryExpr {
                span: DUMMY_SP,
                op: UnaryOp::TypeOf,
                arg: Box::new(Expr::Ident(Ident::new("define".into(), DUMMY_SP, ctxt))),
            })),
            right: Box::new(Expr::Lit(Lit::Str(Str {
                span: DUMMY_SP,
                value: "function".into(),
                raw: None,
            }))),
        })),
        right: Box::new(Expr::Member(MemberExpr {
            span: DUMMY_SP,
            obj: Box::new(Expr::Ident(Ident::new("define".into(), DUMMY_SP, ctxt))),
            prop: MemberProp::Ident(IdentName::new("amd".into(), DUMMY_SP)),
        })),
    })
}

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "webpack5.js", cm.clone()).ok()?;
        detect_from_module_prepared(&module, cm)?.materialize().ok()
    })
}

pub(super) fn detect_from_module_prepared(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> Option<DetectedBundle> {
    let span = tracing::info_span!("webpack5: detect_from_module");
    let _enter = span.enter();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        let Some(bootstrap_body) = extract_iife_body(expr) else {
            continue;
        };
        if let Some(result) = extract_webpack5_modules(bootstrap_body, cm.clone()) {
            return Some(result);
        }
    }
    // Fallback: `output.iife: false` and `experiments.outputModule` emit the
    // bootstrap statements at the top level instead of inside an IIFE.
    detect_webpack5_top_level(module, cm)
}

/// Detect an unwrapped webpack 5 bootstrap (no IIFE): the modules container,
/// require function, runtime, and entry all sit at the file's top level. This
/// is `output.iife: false` (script/CommonJS output).
///
/// `experiments.outputModule` also drops the IIFE, but emits top-level ESM
/// `import`/`export` declarations that carry the library's public surface.
/// Recovering those faithfully needs harmony-export reconstruction the driver
/// pipeline does not yet do, so a bundle with any top-level `ModuleDecl` is
/// left untouched rather than extracted into an entry that silently loses its
/// exports.
fn detect_webpack5_top_level(module: &Module, cm: Lrc<SourceMap>) -> Option<DetectedBundle> {
    let mut stmts: Vec<Stmt> = Vec::with_capacity(module.body.len());
    for item in &module.body {
        match item {
            ModuleItem::Stmt(stmt) => stmts.push(stmt.clone()),
            ModuleItem::ModuleDecl(_) => return None,
        }
    }

    // Guard against arbitrary top-level `var xs = [function(a){}]` arrays:
    // require a modules container the file actually invokes as a module table.
    // (The IIFE path relies on the wrapper itself as the signal.)
    let modules_sym = stmts.iter().find_map(|stmt| {
        let Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) = stmt else {
            return None;
        };
        extract_webpack_modules_container(var_decl).map(|(_, sym)| sym)
    })?;
    if !modules_table_invoked(&stmts, &modules_sym) {
        return None;
    }

    let bootstrap_body = swc_core::ecma::ast::BlockStmt {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        stmts,
    };
    extract_webpack5_modules(&bootstrap_body, cm)
}

pub(super) fn detect_runtime_entry_from_module(
    module: &Module,
    source: &str,
) -> Option<UnpackResult> {
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        let Some(bootstrap_body) = extract_iife_body(expr) else {
            continue;
        };
        if is_webpack5_runtime_entry_body(bootstrap_body) {
            return Some(UnpackResult::new(
                vec![UnpackedModule {
                    id: "entry".to_string(),
                    is_entry: true,
                    code: source.to_string(),
                    filename: "entry.js".to_string(),
                    source_ranges: vec![(0, source.len() as u32)],
                    source_input: String::new(),
                    generated_source_map: Vec::new(),
                }],
                BundleFormat::Webpack5,
            ));
        }
    }
    None
}

/// Detect and extract modules from a webpack5 JSONP chunk format:
/// `(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([[chunkIds], {modules}])`
pub fn detect_and_extract_chunk(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "webpack5.js", cm.clone()).ok()?;
        detect_chunk_from_module_prepared(&module, cm)?
            .materialize()
            .ok()
    })
}

pub(crate) fn detect_chunk_ids_from_module(module: &Module) -> HashSet<usize> {
    let mut ids = HashSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        if let Some((chunk_ids, _)) = extract_chunk_push_parts(expr) {
            ids.extend(numeric_ids_from_array(chunk_ids));
        }
        ids.extend(extract_commonjs_chunk_ids(expr));
    }
    ids
}

pub(super) fn detect_chunk_from_module_prepared(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> Option<DetectedBundle> {
    let span = tracing::info_span!("webpack5: detect_chunk_from_module");
    let _enter = span.enter();
    let mut all_modules = Vec::new();
    let mut all_prepared = Vec::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        if let Some(modules_container) = extract_chunk_push_modules(expr) {
            let (modules, prepared) =
                extract_modules_from_container(&modules_container, cm.clone())?;
            all_modules.extend(modules);
            all_prepared.extend(prepared);
        } else if let Some(modules_container) = extract_commonjs_chunk_modules(expr) {
            let (modules, prepared) =
                extract_modules_from_container(&modules_container, cm.clone())?;
            all_modules.extend(modules);
            all_prepared.extend(prepared);
        }
    }

    if all_modules.is_empty() {
        return None;
    }

    Some(DetectedBundle::new(
        UnpackResult::new(all_modules, BundleFormat::Webpack5),
        all_prepared,
        cm,
    ))
}

/// The webpack 5 modules container. Usually an object keyed by module id, but
/// `Template.getModulesArrayBounds` switches to a sparse array (indices are
/// the ids) when ids are dense numerics, wrapped in `Array(minId).concat([...])`
/// when the smallest id is non-zero.
enum Webpack5ModulesContainer<'a> {
    Object(&'a ObjectLit),
    Array {
        array: &'a ArrayLit,
        id_offset: usize,
    },
}

impl<'a> Webpack5ModulesContainer<'a> {
    fn from_expr(expr: &'a Expr) -> Option<Self> {
        match strip_parens(expr) {
            Expr::Object(object_lit) => {
                is_valid_modules_object(object_lit)?;
                Some(Self::Object(object_lit))
            }
            Expr::Array(array) => {
                is_valid_modules_array(array)?;
                Some(Self::Array {
                    array,
                    id_offset: 0,
                })
            }
            Expr::Call(call) => {
                let (array, id_offset) = split_array_concat(call)?;
                is_valid_modules_array(array)?;
                Some(Self::Array { array, id_offset })
            }
            _ => None,
        }
    }

    fn entry_count(&self) -> usize {
        match self {
            Self::Object(object_lit) => object_lit.props.len(),
            Self::Array { array, .. } => array.elems.len(),
        }
    }
}

/// Whether the modules-table binding is invoked the way the webpack require
/// function invokes a factory: `modules[id](module, module.exports, require)`
/// (webpack 5) or `modules[id].call(module.exports, module, ...)` (webpack 4
/// runtimes some bundles keep). This is the defining relationship between the
/// require function and the module table, so it distinguishes a real table
/// from an ordinary array of functions regardless of how the require function
/// itself is written (declaration, expression, or minified/inlined). It also
/// admits zero-parameter factories, which webpack emits for side-effect-only
/// modules whose (module, exports, require) parameters are all unused.
fn modules_table_invoked(stmts: &[Stmt], modules_sym: &Atom) -> bool {
    let mut finder = ModulesTableInvocationFinder {
        modules_sym,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

/// `expr` is `modules_sym[<computed>]`.
fn is_modules_index(expr: &Expr, modules_sym: &Atom) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !matches!(prop, MemberProp::Computed(_)) {
        return false;
    }
    matches!(strip_parens(obj), Expr::Ident(id) if id.sym == *modules_sym)
}

struct ModulesTableInvocationFinder<'a> {
    modules_sym: &'a Atom,
    found: bool,
}

impl Visit for ModulesTableInvocationFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let callee = strip_parens(callee);
        // `modules[id](module, module.exports, require)` — factory receives at
        // least the module object even when it declares no parameters.
        if is_modules_index(callee, self.modules_sym) && !call.args.is_empty() {
            self.found = true;
            return;
        }
        // `modules[id].call(module.exports, module, module.exports, require)`
        if let Expr::Member(MemberExpr { obj, prop, .. }) = callee {
            if matches!(prop, MemberProp::Ident(ident) if ident.sym.as_ref() == "call")
                && is_modules_index(strip_parens(obj), self.modules_sym)
                && call.args.len() >= 2
            {
                self.found = true;
            }
        }
    }
}

/// Match `Array(<n>).concat([...])` — webpack's sparse-array header when the
/// smallest module id is non-zero. Emitted by both webpack 4 and 5, so the
/// webpack4 unpacker reuses this matcher.
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

/// Match the pattern: `(self.X = self.X || []).push([[ids], {modules}])`
/// or `(window["X"] = window["X"] || []).push([[ids], {modules}])`.
/// The modules payload may also be a sparse array — see
/// [`Webpack5ModulesContainer`].
fn extract_chunk_push_modules(expr: &Expr) -> Option<Webpack5ModulesContainer<'_>> {
    extract_chunk_push_parts(expr).map(|(_, modules)| modules)
}

fn extract_chunk_push_parts(expr: &Expr) -> Option<(&ArrayLit, Webpack5ModulesContainer<'_>)> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    // callee is (self.X = self.X || []).push
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return None;
    };
    let MemberProp::Ident(push_ident) = prop else {
        return None;
    };
    if push_ident.sym.as_ref() != "push" {
        return None;
    }

    // obj is (self.X = self.X || []) — a parenthesized assignment
    let obj = strip_parens(obj);
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        right,
        ..
    }) = obj
    else {
        return None;
    };

    // right side: self.X || []
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        right: or_right,
        ..
    }) = &**right
    else {
        return None;
    };
    // Verify right side of || is an empty array
    let Expr::Array(ArrayLit { elems, .. }) = &**or_right else {
        return None;
    };
    if !elems.is_empty() {
        return None;
    }

    // push argument: [[chunkIds], {modules}, ...]
    if call.args.is_empty() {
        return None;
    }
    let push_arg = &call.args[0].expr;
    let Expr::Array(ArrayLit {
        elems: push_elems, ..
    }) = &**push_arg
    else {
        return None;
    };
    // Must have at least 2 elements: [chunkIds, modulesObject]
    if push_elems.len() < 2 {
        return None;
    }
    // First element: array of chunk IDs
    let Some(Some(first)) = push_elems.first() else {
        return None;
    };
    let Expr::Array(chunk_ids) = &*first.expr else {
        return None;
    };
    // Second element: modules container (object or sparse array)
    let Some(Some(second)) = push_elems.get(1) else {
        return None;
    };
    let modules_container = Webpack5ModulesContainer::from_expr(&second.expr)?;

    Some((chunk_ids, modules_container))
}

/// Match CommonJS async chunk modules:
/// `exports.modules = { ... }`
/// or minified sequence forms like `exports.id=1,exports.modules={...}`.
fn extract_commonjs_chunk_modules(expr: &Expr) -> Option<Webpack5ModulesContainer<'_>> {
    match strip_parens(expr) {
        Expr::Assign(assign) => extract_commonjs_chunk_modules_from_assign(assign),
        Expr::Seq(seq) => seq
            .exprs
            .iter()
            .find_map(|expr| extract_commonjs_chunk_modules(expr)),
        _ => None,
    }
}

fn extract_commonjs_chunk_modules_from_assign(
    assign: &AssignExpr,
) -> Option<Webpack5ModulesContainer<'_>> {
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    let Expr::Ident(obj) = &*member.obj else {
        return None;
    };
    if obj.sym.as_ref() != "exports" {
        return None;
    }
    if !member_prop_name_is(&member.prop, "modules") {
        return None;
    }
    Webpack5ModulesContainer::from_expr(&assign.right)
}

fn extract_commonjs_chunk_ids(expr: &Expr) -> HashSet<usize> {
    let mut ids = HashSet::new();
    match strip_parens(expr) {
        Expr::Assign(assign) => {
            collect_commonjs_chunk_ids_from_assign(assign, &mut ids);
        }
        Expr::Seq(seq) => {
            for expr in &seq.exprs {
                ids.extend(extract_commonjs_chunk_ids(expr));
            }
        }
        _ => {}
    }
    ids
}

fn collect_commonjs_chunk_ids_from_assign(assign: &AssignExpr, out: &mut HashSet<usize>) {
    if assign.op != AssignOp::Assign {
        return;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return;
    };
    let Expr::Ident(obj) = &*member.obj else {
        return;
    };
    if obj.sym.as_ref() != "exports" {
        return;
    }

    if member_prop_name_is(&member.prop, "id") {
        if let Some(id) = numeric_id_from_expr(&assign.right) {
            out.insert(id);
        }
    } else if member_prop_name_is(&member.prop, "ids") {
        let Expr::Array(ids) = strip_parens(&assign.right) else {
            return;
        };
        out.extend(numeric_ids_from_array(ids));
    }
}

fn numeric_ids_from_array(array: &ArrayLit) -> HashSet<usize> {
    array
        .elems
        .iter()
        .filter_map(|elem| elem.as_ref())
        .filter_map(|elem| numeric_id_from_expr(&elem.expr))
        .collect()
}

fn numeric_id_from_expr(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parens(expr) else {
        return None;
    };
    let value = number.value;
    if value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    Some(value as usize)
}

fn member_prop_name_is(prop: &MemberProp, expected: &str) -> bool {
    match prop {
        MemberProp::Ident(ident) => ident.sym.as_ref() == expected,
        MemberProp::Computed(computed) => {
            let Expr::Lit(Lit::Str(value)) = strip_parens(&computed.expr) else {
                return false;
            };
            value.value.as_str() == Some(expected)
        }
        _ => false,
    }
}

/// Extract modules from a modules container where keys (or array indices) are
/// module IDs and values are factory functions.
/// Used by both entry bundles and JSONP chunks.
fn extract_modules_from_container(
    modules_container: &Webpack5ModulesContainer<'_>,
    cm: Lrc<SourceMap>,
) -> Option<(Vec<UnpackedModule>, Vec<Option<PreparedModuleAst>>)> {
    let span = tracing::info_span!(
        "webpack5: extract_modules_from_object",
        count = modules_container.entry_count()
    );
    let _enter = span.enter();

    let module_entries = collect_module_descriptors(modules_container)?;

    let id_to_filename: HashMap<usize, String> = module_entries
        .iter()
        .filter_map(|entry| {
            entry
                .id
                .parse::<usize>()
                .ok()
                .map(|n| (n, entry.filename.clone()))
        })
        .collect();
    let str_id_to_filename: HashMap<String, String> = module_entries
        .iter()
        .map(|entry| (entry.id.clone(), entry.filename.clone()))
        .collect();

    let mut modules = Vec::new();
    let mut prepared = Vec::new();

    for entry in &module_entries {
        let ast = prepare_webpack5_module(entry, &id_to_filename, &str_id_to_filename)?;
        modules.push(UnpackedModule {
            id: entry.id.clone(),
            is_entry: false,
            code: source_fallback_for_stmts(&cm, entry.body_stmts),
            filename: entry.filename.clone(),
            source_ranges: spans_byte_ranges(&cm, entry.body_stmts.iter().map(|s| s.span())),
            source_input: String::new(),
            generated_source_map: Vec::new(),
        });
        prepared.push(Some(ast));
    }

    Some((modules, prepared))
}

fn extract_webpack5_modules(
    bootstrap_body: &swc_core::ecma::ast::BlockStmt,
    cm: Lrc<SourceMap>,
) -> Option<DetectedBundle> {
    let span = tracing::info_span!("webpack5: extract_modules");
    let _enter = span.enter();

    let (modules_container, modules_sym) = {
        let span = tracing::info_span!("webpack5: find modules object");
        let _enter = span.enter();
        let mut found: Option<(Webpack5ModulesContainer<'_>, Atom)> = None;
        for stmt in &bootstrap_body.stmts {
            let Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) = stmt else {
                continue;
            };
            let Some((container, modules_sym)) = extract_webpack_modules_container(var_decl) else {
                continue;
            };
            // An array of functions is ambiguous with ordinary code, so require
            // the defining relationship: the bootstrap invokes it as a module
            // table (`modules[id](module, ...)`). Object containers are keyed by
            // explicit module ids and, inside the IIFE wrapper, are signal
            // enough on their own.
            if matches!(container, Webpack5ModulesContainer::Array { .. })
                && !modules_table_invoked(&bootstrap_body.stmts, &modules_sym)
            {
                continue;
            }
            found = Some((container, modules_sym));
            break;
        }
        found?
    };

    let module_entries = {
        let span = tracing::info_span!("webpack5: collect module entries");
        let _enter = span.enter();
        collect_module_descriptors(&modules_container)?
    };

    let id_to_filename: HashMap<usize, String> = module_entries
        .iter()
        .filter_map(|entry| {
            entry
                .id
                .parse::<usize>()
                .ok()
                .map(|n| (n, entry.filename.clone()))
        })
        .collect();
    let str_id_to_filename: HashMap<String, String> = module_entries
        .iter()
        .map(|entry| (entry.id.clone(), entry.filename.clone()))
        .collect();

    let mut modules = Vec::new();
    let mut prepared = Vec::new();

    {
        let span = tracing::info_span!(
            "webpack5: prepare all modules",
            count = module_entries.len()
        );
        let _enter = span.enter();
        for entry in &module_entries {
            let ast = prepare_webpack5_module(entry, &id_to_filename, &str_id_to_filename)?;
            modules.push(UnpackedModule {
                id: entry.id.clone(),
                is_entry: false,
                code: source_fallback_for_stmts(&cm, entry.body_stmts),
                filename: entry.filename.clone(),
                source_ranges: spans_byte_ranges(&cm, entry.body_stmts.iter().map(|s| s.span())),
                source_input: String::new(),
                generated_source_map: Vec::new(),
            });
            prepared.push(Some(ast));
        }
    }

    // Check for trailing IIFE entry point
    let has_synthetic_entry = if let Some(entry_body) = extract_trailing_entry_body(bootstrap_body)
    {
        let entry_ranges = spans_byte_ranges(&cm, entry_body.iter().map(|s| s.span()));
        let code = emit_webpack5_entry_module(
            entry_body,
            cm.clone(),
            &id_to_filename,
            &str_id_to_filename,
            Atom::from("__webpack_require__"),
            Some(Atom::from("__webpack_exports__")),
        );
        append_synthetic_entry(&mut modules, &mut prepared, entry_ranges, code)
    } else if let Some(entry) = extract_ncc_inline_entry(bootstrap_body) {
        let entry_ranges = spans_byte_ranges(&cm, entry.body_stmts.iter().map(|s| s.span()));
        let code = emit_webpack5_entry_module(
            entry.body_stmts,
            cm.clone(),
            &id_to_filename,
            &str_id_to_filename,
            entry.require_sym,
            None,
        );
        append_synthetic_entry(&mut modules, &mut prepared, entry_ranges, code)
    } else {
        false
    };

    // Fallback: scan bootstrap for entry-module startup calls, then for an
    // inline startup section (`var __webpack_exports__ = {};` + entry code).
    if !has_synthetic_entry {
        if let Some(entry_id) = find_ncc_direct_entry(bootstrap_body)
            .or_else(|| find_require_s_entry(bootstrap_body))
            .or_else(|| find_require_o_entry(bootstrap_body))
        {
            for module in &mut modules {
                if module.id == entry_id {
                    module.is_entry = true;
                    break;
                }
            }
        } else if let Some(startup) = extract_webpack5_inline_startup(bootstrap_body, &modules_sym)
        {
            let entry_ranges = spans_byte_ranges(&cm, startup.body_stmts.iter().map(|s| s.span()));
            let code = emit_webpack5_entry_module(
                startup.body_stmts,
                cm.clone(),
                &id_to_filename,
                &str_id_to_filename,
                startup.require_sym,
                startup.exports_sym,
            );
            append_synthetic_entry(&mut modules, &mut prepared, entry_ranges, code);
        }
    }

    if modules.is_empty() {
        return None;
    }

    Some(DetectedBundle::new(
        UnpackResult::new(modules, BundleFormat::Webpack5),
        prepared,
        cm,
    ))
}

fn append_synthetic_entry(
    modules: &mut Vec<UnpackedModule>,
    prepared: &mut Vec<Option<PreparedModuleAst>>,
    source_ranges: Vec<(u32, u32)>,
    code: Option<String>,
) -> bool {
    let Some(code) = code else {
        return false;
    };
    modules.push(UnpackedModule {
        id: "entry".to_string(),
        is_entry: true,
        code,
        filename: "entry.js".to_string(),
        source_ranges,
        source_input: String::new(),
        generated_source_map: Vec::new(),
    });
    prepared.push(None);
    true
}

fn emit_webpack5_entry_module(
    body_stmts: Vec<Stmt>,
    cm: Lrc<SourceMap>,
    id_to_filename: &HashMap<usize, String>,
    str_id_to_filename: &HashMap<String, String>,
    require_sym: Atom,
    exports_sym: Option<Atom>,
) -> Option<String> {
    let (mut synthetic_module, _) = normalize_extracted_webpack_entry_module(
        body_stmts,
        id_to_filename,
        str_id_to_filename,
        require_sym,
        exports_sym,
    );
    apply_fixer(&mut synthetic_module).ok()?;
    emit_module(&synthetic_module, cm).ok()
}

/// Normalize a webpack5 runtime entry body into a standalone module.
///
/// This is extraction normalization only: it rewrites webpack runtime
/// identifiers, module ids, and runtime helper noise, then leaves ESM recovery
/// and readability cleanup to the driver pipeline.
fn normalize_extracted_webpack_entry_module(
    body_stmts: Vec<Stmt>,
    id_to_filename: &HashMap<usize, String>,
    str_id_to_filename: &HashMap<String, String>,
    require_sym: Atom,
    exports_sym: Option<Atom>,
) -> (Module, Mark) {
    let mut synthetic_module = build_module_from_stmts(body_stmts);
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    synthetic_module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);

    let mut id_rewriter = RequireIdRewriter {
        require_sym: require_sym.clone(),
        unresolved_mark,
        from_filename: "entry.js",
        id_to_filename,
    };
    synthetic_module.visit_mut_with(&mut id_rewriter);
    let mut str_rewriter = RequireStringIdRewriter {
        require_sym: require_sym.clone(),
        unresolved_mark,
        from_filename: "entry.js",
        id_to_filename: str_id_to_filename,
    };
    synthetic_module.visit_mut_with(&mut str_rewriter);

    rewrite_require_n_accesses(&mut synthetic_module, require_sym.clone(), unresolved_mark);

    replace_ident(
        &mut synthetic_module,
        (require_sym.clone(), unresolved_ctxt),
        &Ident::new(Atom::from("require"), Default::default(), unresolved_ctxt),
    );
    if let Some(exports_sym) = exports_sym {
        replace_ident(
            &mut synthetic_module,
            (exports_sym, unresolved_ctxt),
            &Ident::new(Atom::from("exports"), Default::default(), unresolved_ctxt),
        );
    }

    let mut normalizer = Webpack5RuntimeNormalizer {
        require_sym: Atom::from("require"),
        unresolved_mark,
    };
    synthetic_module.visit_mut_with(&mut normalizer);

    (synthetic_module, unresolved_mark)
}

fn extract_trailing_entry_body(
    bootstrap_body: &swc_core::ecma::ast::BlockStmt,
) -> Option<Vec<Stmt>> {
    bootstrap_body
        .stmts
        .iter()
        .rev()
        .find(|stmt| !matches!(stmt, Stmt::Return(_)))
        .and_then(extract_iife_stmt_body)
        .map(|body| body.stmts.clone())
}

struct NccInlineEntry {
    body_stmts: Vec<Stmt>,
    require_sym: Atom,
}

/// Extract ncc's inline startup program.
///
/// ncc uses webpack 5's module table but emits the entry directly in the
/// bootstrap IIFE rather than in a trailing IIFE or a `require.s`/`require.O`
/// startup. Its post-processing gives the runtime require binding a stable
/// `nccwpck_require` marker. The entry starts at the declaration of the binding
/// assigned to `module.exports` by the final bootstrap statement; unlike the
/// generated variable name, that export assignment survives minification.
fn extract_ncc_inline_entry(
    bootstrap_body: &swc_core::ecma::ast::BlockStmt,
) -> Option<NccInlineEntry> {
    let require_sym = find_ncc_require_sym(bootstrap_body)?;

    let exports_sym = bootstrap_body
        .stmts
        .iter()
        .rev()
        .find_map(ncc_module_exports_binding)?;
    let entry_start = bootstrap_body
        .stmts
        .iter()
        .enumerate()
        .find_map(|(index, stmt)| {
            let Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) = stmt else {
                return None;
            };
            var_decl
                .decls
                .iter()
                .any(|decl| matches!(&decl.name, Pat::Ident(binding) if binding.sym == exports_sym))
                .then_some(index)
        })?;

    Some(NccInlineEntry {
        body_stmts: bootstrap_body.stmts[entry_start..].to_vec(),
        require_sym,
    })
}

struct Webpack5InlineStartup {
    body_stmts: Vec<Stmt>,
    require_sym: Atom,
    exports_sym: Option<Atom>,
}

/// Extract webpack 5's inline startup: when the entry module needs no IIFE
/// wrapper and no `require.s`/`require.O` startup, the runtime inlines the entry
/// statements at the end of the bootstrap.
///
/// In unminified output an empty-object var declaration
/// (`var __webpack_exports__ = {};`) anchors the startup section. It sits after
/// the require function (which rules out the module cache — webpack declares it
/// before the require function) with no webpack runtime definitions following
/// it (which rules out runtime-section state like load-script's
/// `var inProgress = {};`). Minifiers may merge the entry's leading declarators
/// into the anchor statement (`var o = {}, e = r(9);`), so the anchor's
/// trailing declarators are kept as entry code.
///
/// Minifiers also drop the anchor entirely because nothing reads it. In that
/// case everything after the last runtime definition is treated as the entry,
/// provided it calls the require binding.
fn extract_webpack5_inline_startup(
    bootstrap_body: &swc_core::ecma::ast::BlockStmt,
    modules_sym: &Atom,
) -> Option<Webpack5InlineStartup> {
    let (require_idx, require_sym) = find_webpack5_require_fn(&bootstrap_body.stmts, modules_sym)?;

    // Runtime sections define helpers as assignments to require properties
    // (`require.d = ...`, `require.f.j = ...`). The startup section is the
    // only thing emitted after the last of them.
    let last_runtime_idx = bootstrap_body
        .stmts
        .iter()
        .rposition(|stmt| contains_require_member_assignment(stmt, &require_sym));

    let startup_scan_from = last_runtime_idx
        .map(|idx| idx + 1)
        .unwrap_or(require_idx + 1)
        .max(require_idx + 1);

    let entry_region = &bootstrap_body.stmts[startup_scan_from..];
    if entry_region.is_empty() {
        return None;
    }

    // The anchor, when present, is always the *first* statement of the startup
    // section (webpack emits it immediately after the runtime). Only inspect
    // that statement — scanning deeper would mistake an ordinary entry local
    // such as `const app = {}` for the anchor and discard the statements before
    // it (e.g. a real `const dep = require(1)` import).
    let (body_stmts, exports_sym) = match empty_object_anchor(&entry_region[0]) {
        Some((binding, var_decl)) => {
            let mut body_stmts: Vec<Stmt> = Vec::new();
            if var_decl.decls.len() > 1 {
                let mut rest = var_decl.clone();
                rest.decls.remove(0);
                body_stmts.push(Stmt::Decl(swc_core::ecma::ast::Decl::Var(Box::new(rest))));
            }
            body_stmts.extend(entry_region[1..].iter().cloned());
            (body_stmts, Some(binding))
        }
        // No anchor (minified: the unused anchor is dropped): the whole region
        // is the entry.
        None => (entry_region.to_vec(), None),
    };

    // Require the recovered entry to actually call the require binding so a
    // bundle with no startup does not synthesize a bogus entry.
    if body_stmts.is_empty() || !stmts_call_require(&body_stmts, &require_sym) {
        return None;
    }
    Some(Webpack5InlineStartup {
        body_stmts,
        require_sym,
        exports_sym,
    })
}

/// If `stmt` begins with an empty-object declarator (`var x = {}` — webpack's
/// `__webpack_exports__` anchor), return its binding symbol and the var decl.
fn empty_object_anchor(stmt: &Stmt) -> Option<(Atom, &VarDecl)> {
    let Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) = stmt else {
        return None;
    };
    let first = var_decl.decls.first()?;
    let Pat::Ident(binding) = &first.name else {
        return None;
    };
    let init = first.init.as_ref()?;
    let Expr::Object(object) = strip_parens(init) else {
        return None;
    };
    if !object.props.is_empty() {
        return None;
    }
    Some((binding.id.sym.clone(), var_decl))
}

/// Whether any statement calls the require binding, e.g. `r(1)`.
fn stmts_call_require(stmts: &[Stmt], require_sym: &Atom) -> bool {
    let mut finder = RequireCallFinder {
        require_sym,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

struct RequireCallFinder<'a> {
    require_sym: &'a Atom,
    found: bool,
}

impl Visit for RequireCallFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(ident) = strip_parens(callee) {
                if ident.sym == *self.require_sym {
                    self.found = true;
                }
            }
        }
    }
}

/// Find the webpack require function: the function declaration whose body
/// references the modules container binding.
fn find_webpack5_require_fn(stmts: &[Stmt], modules_sym: &Atom) -> Option<(usize, Atom)> {
    stmts.iter().enumerate().find_map(|(idx, stmt)| {
        let Stmt::Decl(swc_core::ecma::ast::Decl::Fn(fn_decl)) = stmt else {
            return None;
        };
        let mut finder = SymUsageFinder {
            sym: modules_sym,
            found: false,
        };
        fn_decl.function.visit_with(&mut finder);
        finder.found.then(|| (idx, fn_decl.ident.sym.clone()))
    })
}

struct SymUsageFinder<'a> {
    sym: &'a Atom,
    found: bool,
}

impl Visit for SymUsageFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == *self.sym {
            self.found = true;
        }
    }
}

/// Whether the statement assigns to a (possibly nested) member of the require
/// binding, e.g. `require.d = ...` or `require.f.j = ...` — the shape of
/// webpack runtime-section definitions.
fn contains_require_member_assignment(stmt: &Stmt, require_sym: &Atom) -> bool {
    let mut finder = RequireMemberAssignFinder {
        require_sym,
        found: false,
    };
    stmt.visit_with(&mut finder);
    finder.found
}

struct RequireMemberAssignFinder<'a> {
    require_sym: &'a Atom,
    found: bool,
}

impl Visit for RequireMemberAssignFinder<'_> {
    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        assign.visit_children_with(self);
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
            return;
        };
        let mut obj = &*member.obj;
        while let Expr::Member(inner) = strip_parens(obj) {
            obj = &inner.obj;
        }
        if let Expr::Ident(root) = strip_parens(obj) {
            if root.sym == *self.require_sym {
                self.found = true;
            }
        }
    }
}

fn find_ncc_require_sym(bootstrap_body: &swc_core::ecma::ast::BlockStmt) -> Option<Atom> {
    bootstrap_body.stmts.iter().find_map(|stmt| {
        let Stmt::Decl(swc_core::ecma::ast::Decl::Fn(function)) = stmt else {
            return None;
        };
        function
            .ident
            .sym
            .as_ref()
            .contains("nccwpck_require")
            .then(|| function.ident.sym.clone())
    })
}

/// Recognize ncc's direct CommonJS startup:
/// `module.exports = __nccwpck_require__(<entry id>)`.
fn find_ncc_direct_entry(bootstrap_body: &swc_core::ecma::ast::BlockStmt) -> Option<String> {
    let require_sym = find_ncc_require_sym(bootstrap_body)?;
    bootstrap_body
        .stmts
        .iter()
        .rev()
        .find(|statement| !matches!(statement, Stmt::Empty(_)))
        .and_then(|statement| ncc_direct_entry_from_statement(statement, &require_sym))
}

fn ncc_direct_entry_from_statement(statement: &Stmt, require_sym: &Atom) -> Option<String> {
    let Stmt::Expr(ExprStmt { expr, .. }) = statement else {
        return None;
    };
    ncc_direct_entry_from_expr(expr, require_sym)
}

fn ncc_direct_entry_from_expr(expression: &Expr, require_sym: &Atom) -> Option<String> {
    match strip_parens(expression) {
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
                return None;
            };
            let Expr::Ident(module) = strip_parens(&member.obj) else {
                return None;
            };
            if module.sym.as_ref() != "module" || !member_prop_name_is(&member.prop, "exports") {
                return None;
            }
            extract_require_call_id(&assign.right, require_sym)
        }
        Expr::Seq(sequence) => sequence
            .exprs
            .iter()
            .rev()
            .find_map(|expression| ncc_direct_entry_from_expr(expression, require_sym)),
        _ => None,
    }
}

fn ncc_module_exports_binding(statement: &Stmt) -> Option<Atom> {
    let Stmt::Expr(ExprStmt { expr, .. }) = statement else {
        return None;
    };
    ncc_module_exports_binding_from_expr(expr)
}

fn ncc_module_exports_binding_from_expr(expression: &Expr) -> Option<Atom> {
    match strip_parens(expression) {
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
                return None;
            };
            let Expr::Ident(module) = strip_parens(&member.obj) else {
                return None;
            };
            if module.sym.as_ref() != "module" || !member_prop_name_is(&member.prop, "exports") {
                return None;
            }
            let Expr::Ident(exports) = strip_parens(&assign.right) else {
                return None;
            };
            Some(exports.sym.clone())
        }
        Expr::Seq(sequence) => sequence
            .exprs
            .iter()
            .rev()
            .find_map(|expression| ncc_module_exports_binding_from_expr(expression)),
        _ => None,
    }
}

fn is_webpack5_runtime_entry_body(body: &swc_core::ecma::ast::BlockStmt) -> bool {
    // Cheap pre-check: scan only the direct statements of the IIFE body for
    // member assignments `obj.e =`, `obj.u =`, `obj.t =`, and `obj.m =` or
    // `obj.f =` on the same identifier.  This rejects non-runtime IIFEs
    // without a full recursive traversal.
    if !has_runtime_property_assignments(body) {
        return false;
    }

    let mut collector = RuntimePropCollector::default();
    body.visit_with(&mut collector);
    collector.props_by_object.iter().any(|(object, props)| {
        collector.function_names.contains(object)
            && collector.async_chunk_load_objects.contains(object)
            && props.contains("e")
            && props.contains("u")
            && props.contains("t")
            && (props.contains("m") || props.contains("f"))
    })
}

fn has_runtime_property_assignments(body: &swc_core::ecma::ast::BlockStmt) -> bool {
    let mut bits_by_object: HashMap<Atom, u8> = HashMap::new();
    for stmt in &body.stmts {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            continue;
        };
        if check_runtime_assign(expr, &mut bits_by_object) {
            return true;
        }
        // Minified bundles pack assignments into comma sequences:
        //   f.m=o, f.t=function(){...}, f.e=..., f.u=...
        if let Expr::Seq(SeqExpr { exprs, .. }) = &**expr {
            for sub in exprs {
                if check_runtime_assign(sub, &mut bits_by_object) {
                    return true;
                }
            }
        }
    }
    false
}

fn check_runtime_assign(expr: &Expr, bits_by_object: &mut HashMap<Atom, u8>) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return false;
    };
    let Expr::Ident(obj) = &*member.obj else {
        return false;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return false;
    };
    let flag = match prop.sym.as_str() {
        "e" => 1u8,
        "u" => 2,
        "t" => 4,
        "m" | "f" => 8,
        _ => return false,
    };
    let bits = bits_by_object.entry(obj.sym.clone()).or_default();
    *bits |= flag;
    *bits & 0b1111 == 0b1111
}

#[derive(Default)]
struct RuntimePropCollector {
    props_by_object: HashMap<String, HashSet<String>>,
    function_names: HashSet<String>,
    async_chunk_load_objects: HashSet<String>,
}

impl Visit for RuntimePropCollector {
    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        self.function_names.insert(decl.ident.sym.to_string());
        decl.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let Pat::Ident(binding) = &declarator.name {
            if matches!(
                declarator.init.as_deref().map(strip_parens),
                Some(Expr::Fn(_) | Expr::Arrow(_))
            ) {
                self.function_names.insert(binding.id.sym.to_string());
            }
        }

        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
            self.collect_member_prop(member);
        }

        assign.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(object) = self.extract_async_chunk_load_object(call) {
            self.async_chunk_load_objects.insert(object);
        }

        call.visit_children_with(self);
    }
}

impl RuntimePropCollector {
    fn collect_member_prop(&mut self, member: &MemberExpr) {
        let Expr::Ident(obj) = &*member.obj else {
            return;
        };
        let MemberProp::Ident(prop) = &member.prop else {
            return;
        };
        self.props_by_object
            .entry(obj.sym.to_string())
            .or_default()
            .insert(prop.sym.to_string());
    }

    fn extract_async_chunk_load_object(&self, call: &CallExpr) -> Option<String> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr {
            obj: then_obj,
            prop: then_prop,
            ..
        }) = strip_parens(callee)
        else {
            return None;
        };
        if !member_prop_name_is(then_prop, "then") {
            return None;
        }

        let Expr::Call(load_call) = strip_parens(then_obj) else {
            return None;
        };
        let load_object = callee_object_for_member_call(load_call, "e")?;
        let then_arg = call.args.first()?;
        if then_arg.spread.is_some() {
            return None;
        }
        let Expr::Call(bind_call) = strip_parens(&then_arg.expr) else {
            return None;
        };
        let bind_object = self.extract_runtime_t_bind_object(bind_call)?;

        (load_object == bind_object).then_some(load_object)
    }

    fn extract_runtime_t_bind_object(&self, call: &CallExpr) -> Option<String> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr {
            obj: bind_obj,
            prop: bind_prop,
            ..
        }) = strip_parens(callee)
        else {
            return None;
        };
        if !member_prop_name_is(bind_prop, "bind") {
            return None;
        }

        let Expr::Member(MemberExpr {
            obj: t_obj,
            prop: t_prop,
            ..
        }) = strip_parens(bind_obj)
        else {
            return None;
        };
        if !member_prop_name_is(t_prop, "t") {
            return None;
        }
        let Expr::Ident(runtime_ident) = strip_parens(t_obj) else {
            return None;
        };

        let this_arg = call.args.first()?;
        if this_arg.spread.is_some() {
            return None;
        }
        let Expr::Ident(this_ident) = strip_parens(&this_arg.expr) else {
            return None;
        };
        (runtime_ident.sym == this_ident.sym).then(|| runtime_ident.sym.to_string())
    }
}

fn callee_object_for_member_call(call: &CallExpr, expected_prop: &str) -> Option<String> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee) else {
        return None;
    };
    if !member_prop_name_is(prop, expected_prop) {
        return None;
    }
    let Expr::Ident(object) = strip_parens(obj) else {
        return None;
    };
    Some(object.sym.to_string())
}

/// Extract a string module ID from any `PropName` variant.
fn extract_module_id_from_prop_name(key: &PropName) -> Option<String> {
    match key {
        PropName::Str(s) => Some(s.value.as_str().unwrap_or("unknown").to_string()),
        PropName::Num(n) => Some(format!("{}", n.value as i64)),
        PropName::Ident(i) => Some(i.sym.to_string()),
        _ => None,
    }
}

struct Webpack5ModuleDescriptor<'a> {
    id: String,
    filename: String,
    params: Webpack5FactoryParams<'a>,
    body_stmts: &'a [Stmt],
}

enum Webpack5FactoryParams<'a> {
    Function(&'a [swc_core::ecma::ast::Param]),
    Arrow(&'a [Pat]),
}

fn is_valid_modules_object(modules_object: &ObjectLit) -> Option<()> {
    if modules_object.props.is_empty() {
        return None;
    }
    modules_object
        .props
        .iter()
        .all(|prop| module_descriptor_from_prop(prop).is_some())
        .then_some(())
}

/// A sparse module array: every present element must be a factory function or
/// a `false` placeholder (webpack renders a suppressed module source as the
/// literal `false`), and at least one factory must exist.
fn is_valid_modules_array(array: &ArrayLit) -> Option<()> {
    let mut has_factory = false;
    for elem in array.elems.iter().flatten() {
        if elem.spread.is_some() {
            return None;
        }
        if is_false_module_placeholder(&elem.expr) {
            continue;
        }
        extract_factory_parts(&elem.expr)?;
        has_factory = true;
    }
    has_factory.then_some(())
}

fn is_false_module_placeholder(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Lit(Lit::Bool(lit)) if !lit.value)
}

fn collect_module_descriptors<'a>(
    modules_container: &Webpack5ModulesContainer<'a>,
) -> Option<Vec<Webpack5ModuleDescriptor<'a>>> {
    match modules_container {
        Webpack5ModulesContainer::Object(modules_object) => {
            if modules_object.props.is_empty() {
                return None;
            }
            modules_object
                .props
                .iter()
                .map(module_descriptor_from_prop)
                .collect()
        }
        Webpack5ModulesContainer::Array { array, id_offset } => {
            let mut descriptors = Vec::new();
            for (index, elem) in array.elems.iter().enumerate() {
                let Some(elem) = elem else {
                    continue; // array hole
                };
                if elem.spread.is_some() {
                    return None;
                }
                if is_false_module_placeholder(&elem.expr) {
                    continue;
                }
                let (params, body_stmts) = extract_factory_parts(&elem.expr)?;
                let module_id = (id_offset + index).to_string();
                descriptors.push(Webpack5ModuleDescriptor {
                    filename: descriptor_filename(&module_id),
                    id: module_id,
                    params,
                    body_stmts,
                });
            }
            if descriptors.is_empty() {
                return None;
            }
            Some(descriptors)
        }
    }
}

/// Borrow the factory function from a prop, handling both `Prop::KeyValue` and `Prop::Method`.
fn module_descriptor_from_prop(prop: &PropOrSpread) -> Option<Webpack5ModuleDescriptor<'_>> {
    let PropOrSpread::Prop(prop) = prop else {
        return None;
    };
    let (module_id, params, body_stmts) = match &**prop {
        Prop::KeyValue(key_value) => {
            let module_id = extract_module_id_from_prop_name(&key_value.key)?;
            let (params, body_stmts) = extract_factory_parts(&key_value.value)?;
            (module_id, params, body_stmts)
        }
        Prop::Method(method) => {
            let module_id = extract_module_id_from_prop_name(&method.key)?;
            let body = method.function.body.as_ref()?;
            (
                module_id,
                Webpack5FactoryParams::Function(&method.function.params),
                body.stmts.as_slice(),
            )
        }
        _ => return None,
    };
    Some(Webpack5ModuleDescriptor {
        filename: descriptor_filename(&module_id),
        id: module_id,
        params,
        body_stmts,
    })
}

fn descriptor_filename(module_id: &str) -> String {
    if module_id.contains('/') || module_id.contains('.') {
        sanitize_filename(module_id)
    } else {
        format!("module-{module_id}.js")
    }
}

fn extract_webpack_modules_container(
    var_decl: &VarDecl,
) -> Option<(Webpack5ModulesContainer<'_>, Atom)> {
    for decl in &var_decl.decls {
        let VarDeclarator {
            name: Pat::Ident(binding),
            init: Some(init),
            ..
        } = decl
        else {
            continue;
        };
        let Some(container) = Webpack5ModulesContainer::from_expr(init) else {
            continue;
        };
        return Some((container, binding.id.sym.clone()));
    }
    None
}

fn extract_factory_parts(expr: &Expr) -> Option<(Webpack5FactoryParams<'_>, &[Stmt])> {
    match strip_parens(expr) {
        Expr::Fn(FnExpr { function, .. }) => {
            let body = function.body.as_ref()?;
            Some((
                Webpack5FactoryParams::Function(&function.params),
                body.stmts.as_slice(),
            ))
        }
        Expr::Arrow(arrow) => {
            let swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) = &*arrow.body else {
                return None;
            };
            Some((
                Webpack5FactoryParams::Arrow(&arrow.params),
                body.stmts.as_slice(),
            ))
        }
        _ => None,
    }
}

fn prepare_webpack5_module(
    descriptor: &Webpack5ModuleDescriptor<'_>,
    id_to_filename: &HashMap<usize, String>,
    str_id_to_filename: &HashMap<String, String>,
) -> Option<PreparedModuleAst> {
    let span = tracing::info_span!("webpack5: prepare_module");
    let _enter = span.enter();

    let globals = Globals::new();
    let (synthetic_module, unresolved_mark) = GLOBALS.set(&globals, || {
        let (mut synthetic_module, unresolved_mark) =
            normalize_extracted_webpack_module(descriptor, id_to_filename, str_id_to_filename)?;
        let span = tracing::info_span!("webpack5: fixer");
        let _enter = span.enter();
        apply_fixer(&mut synthetic_module).ok()?;
        Some((synthetic_module, unresolved_mark))
    })?;

    Some(PreparedModuleAst {
        globals,
        module: synthetic_module,
        unresolved_mark,
        recoverable_parse_errors: Vec::new(),
    })
}

/// Normalize a webpack5 factory body into a standalone module.
///
/// This does only bundler-format work: standardizes factory parameter names,
/// rewrites extracted module references, expands `require.n`, and removes
/// webpack runtime helper/decorator noise. General decompiler rules run later
/// in the driver pipeline.
fn normalize_extracted_webpack_module(
    descriptor: &Webpack5ModuleDescriptor<'_>,
    id_to_filename: &HashMap<usize, String>,
    str_id_to_filename: &HashMap<String, String>,
) -> Option<(Module, Mark)> {
    let mut synthetic_module = build_module_from_stmts(descriptor.body_stmts.to_vec());

    let param_syms: Vec<Atom> = match descriptor.params {
        Webpack5FactoryParams::Function(params) => params
            .iter()
            .filter_map(|p| match &p.pat {
                Pat::Ident(binding) => Some(binding.sym.clone()),
                _ => None,
            })
            .collect(),
        Webpack5FactoryParams::Arrow(params) => params
            .iter()
            .filter_map(|p| match p {
                Pat::Ident(binding) => Some(binding.sym.clone()),
                _ => None,
            })
            .collect(),
    };

    let unresolved_mark = {
        let span = tracing::info_span!("webpack5: resolver");
        let _enter = span.enter();
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        synthetic_module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        unresolved_mark
    };

    {
        let span = tracing::info_span!("webpack5: normalize");
        let _enter = span.enter();
        let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
        let renames = param_syms
            .iter()
            .zip(["module", "exports", "require"])
            .filter(|(source, target)| source.as_ref() != *target)
            .map(|(source, target)| BindingRename {
                old: (source.clone(), unresolved_ctxt),
                new: target.into(),
            })
            .collect::<Vec<_>>();
        if !deconflict_runtime_binding_renames(&mut synthetic_module, &renames) {
            return None;
        }
        for rename in &renames {
            let to_ident = Ident::new(rename.new.clone(), Default::default(), unresolved_ctxt);
            replace_ident(&mut synthetic_module, rename.old.clone(), &to_ident);
        }

        let require_sym = Atom::from("require");

        let mut id_rewriter = RequireIdRewriter {
            require_sym: require_sym.clone(),
            unresolved_mark,
            from_filename: &descriptor.filename,
            id_to_filename,
        };
        synthetic_module.visit_mut_with(&mut id_rewriter);
        let mut str_rewriter = RequireStringIdRewriter {
            require_sym: require_sym.clone(),
            unresolved_mark,
            from_filename: &descriptor.filename,
            id_to_filename: str_id_to_filename,
        };
        synthetic_module.visit_mut_with(&mut str_rewriter);

        rewrite_require_n_accesses(&mut synthetic_module, require_sym.clone(), unresolved_mark);

        let mut normalizer = Webpack5RuntimeNormalizer {
            require_sym,
            unresolved_mark,
        };
        synthetic_module.visit_mut_with(&mut normalizer);
    }

    Some((synthetic_module, unresolved_mark))
}

fn extract_iife_stmt_body(stmt: &Stmt) -> Option<&swc_core::ecma::ast::BlockStmt> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    extract_iife_body(expr)
}

fn extract_iife_body(expr: &Expr) -> Option<&swc_core::ecma::ast::BlockStmt> {
    match expr {
        Expr::Call(call) => extract_callee_body(&call.callee),
        Expr::Unary(unary) if matches!(unary.op, swc_core::ecma::ast::UnaryOp::Bang) => {
            let Expr::Call(call) = &*unary.arg else {
                return None;
            };
            extract_callee_body(&call.callee)
        }
        _ => None,
    }
}

fn extract_callee_body(callee: &Callee) -> Option<&swc_core::ecma::ast::BlockStmt> {
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    match &**callee_expr {
        Expr::Fn(FnExpr { function, .. }) => function.body.as_ref(),
        Expr::Arrow(arrow) => match &*arrow.body {
            swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => Some(body),
            _ => None,
        },
        Expr::Paren(paren) => match &*paren.expr {
            Expr::Fn(FnExpr { function, .. }) => function.body.as_ref(),
            Expr::Arrow(arrow) => match &*arrow.body {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => Some(body),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn sanitize_filename(module_id: &str) -> String {
    crate::unpacker::sanitize_relative_path(module_id, "unknown.js")
}

/// Scan the bootstrap body for `__webpack_require__(__webpack_require__.s = <id>)` and return
/// the entry module ID as a string.
fn find_require_s_entry(body: &swc_core::ecma::ast::BlockStmt) -> Option<String> {
    for stmt in &body.stmts {
        if let Some(id) = find_require_s_in_stmt(stmt) {
            return Some(id);
        }
    }
    None
}

/// Check a single statement for the `require(require.s = <id>)` pattern.
/// Matches both standalone expression statements and `var x = require(require.s = <id>)`.
fn find_require_s_in_stmt(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => find_require_s_in_expr(expr),
        Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) => {
            for decl in &var_decl.decls {
                if let Some(init) = &decl.init {
                    if let Some(id) = find_require_s_in_expr(init) {
                        return Some(id);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Check an expression for `require(require.s = <id>)` and extract the module ID.
fn find_require_s_in_expr(expr: &Expr) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }
    // The argument should be `require.s = <id>`
    let Expr::Assign(assign) = &*call.args[0].expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    // Left side: require.s (a member expression)
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    let MemberProp::Ident(prop_ident) = &member.prop else {
        return None;
    };
    if prop_ident.sym.as_ref() != "s" {
        return None;
    }
    // Right side: the module ID (numeric literal or string)
    match &*assign.right {
        Expr::Lit(Lit::Num(n)) => Some(format!("{}", n.value as i64)),
        Expr::Lit(Lit::Str(s)) => Some(s.value.as_str().unwrap_or("unknown").to_string()),
        _ => None,
    }
}

/// Scan the bootstrap body for webpack 5 runtime startup:
/// `require.O(void 0, [chunkId], function() { return require(<id>); })`.
fn find_require_o_entry(body: &swc_core::ecma::ast::BlockStmt) -> Option<String> {
    for stmt in &body.stmts {
        if let Some(id) = find_require_o_in_stmt(stmt) {
            return Some(id);
        }
    }
    None
}

fn find_require_o_in_stmt(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => find_require_o_in_expr(expr),
        Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) => {
            for decl in &var_decl.decls {
                if let Some(init) = &decl.init {
                    if let Some(id) = find_require_o_in_expr(init) {
                        return Some(id);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn find_require_o_in_expr(expr: &Expr) -> Option<String> {
    match strip_parens(expr) {
        Expr::Call(call) => extract_require_o_entry_call(call),
        Expr::Assign(assign) => find_require_o_in_expr(&assign.right),
        Expr::Seq(seq) => seq
            .exprs
            .iter()
            .find_map(|expr| find_require_o_in_expr(expr)),
        _ => None,
    }
}

fn extract_require_o_entry_call(call: &CallExpr) -> Option<String> {
    let require_sym = extract_require_o_callee_sym(&call.callee)?;
    if call.args.len() < 3 {
        return None;
    }
    extract_require_call_from_callback(&call.args[2].expr, require_sym)
}

fn extract_require_o_callee_sym(callee: &Callee) -> Option<&Atom> {
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return None;
    };
    let MemberProp::Ident(prop_ident) = prop else {
        return None;
    };
    if prop_ident.sym.as_ref() != "O" {
        return None;
    }
    let Expr::Ident(require_ident) = &**obj else {
        return None;
    };
    Some(&require_ident.sym)
}

fn extract_require_call_from_callback(expr: &Expr, require_sym: &Atom) -> Option<String> {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => {
            let body = fn_expr.function.body.as_ref()?;
            extract_require_call_from_body(body, require_sym)
        }
        Expr::Arrow(arrow) => match &*arrow.body {
            BlockStmtOrExpr::BlockStmt(body) => extract_require_call_from_body(body, require_sym),
            BlockStmtOrExpr::Expr(expr) => extract_require_call_id(expr, require_sym),
        },
        _ => None,
    }
}

fn extract_require_call_from_body(
    body: &swc_core::ecma::ast::BlockStmt,
    require_sym: &Atom,
) -> Option<String> {
    if body.stmts.len() != 1 {
        return None;
    }
    let Stmt::Return(ret) = &body.stmts[0] else {
        return None;
    };
    let arg = ret.arg.as_ref()?;
    extract_require_call_id(arg, require_sym)
}

fn extract_require_call_id(expr: &Expr, require_sym: &Atom) -> Option<String> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Ident(callee_ident) = &**callee_expr else {
        return None;
    };
    if &callee_ident.sym != require_sym || call.args.len() != 1 {
        return None;
    }
    match &*call.args[0].expr {
        Expr::Lit(Lit::Num(n)) => Some(format!("{}", n.value as i64)),
        Expr::Lit(Lit::Str(s)) => Some(s.value.as_str().unwrap_or("unknown").to_string()),
        _ => None,
    }
}

fn build_module_from_stmts(stmts: Vec<Stmt>) -> Module {
    Module {
        span: Default::default(),
        body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    }
}

fn emit_module(module: &Module, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    emit_module_with_source_map(module, cm).map(|(code, _)| code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_first_var_init(source: &str) -> Box<Expr> {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let module = super::super::parse_es_module(source, "test.js", cm.clone())
                .expect("source should parse");
            let ModuleItem::Stmt(Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl))) =
                &module.body[0]
            else {
                panic!("expected first statement to be var declaration");
            };
            var_decl.decls[0]
                .init
                .clone()
                .expect("declarator should have init")
        })
    }

    fn parse_modules_object(source: &str) -> ObjectLit {
        let init = parse_first_var_init(source);
        let Expr::Object(object) = strip_parens(&init) else {
            panic!("expected object literal init");
        };
        object.clone()
    }

    #[test]
    fn descriptors_accept_function_arrow_and_method_factories() {
        let object = parse_modules_object(
            r#"
const modules = {
  1: function(module, exports, require) { exports.a = require(2); },
  2: (module, exports, require) => { exports.b = 1; },
  3(module, exports, require) { exports.c = 2; }
};
"#,
        );

        let descriptors = collect_module_descriptors(&Webpack5ModulesContainer::Object(&object))
            .expect("descriptors should collect");
        assert_eq!(descriptors.len(), 3);
        assert_eq!(descriptors[0].id, "1");
        assert_eq!(descriptors[1].id, "2");
        assert_eq!(descriptors[2].id, "3");
        assert!(matches!(
            descriptors[0].params,
            Webpack5FactoryParams::Function(_)
        ));
        assert!(matches!(
            descriptors[1].params,
            Webpack5FactoryParams::Arrow(_)
        ));
        assert!(matches!(
            descriptors[2].params,
            Webpack5FactoryParams::Function(_)
        ));
    }

    #[test]
    fn descriptors_reject_concise_arrow_and_non_function_props() {
        let concise_arrow = parse_modules_object(
            r#"
const modules = {
  1: (module, exports, require) => exports.a
};
"#,
        );
        assert!(
            collect_module_descriptors(&Webpack5ModulesContainer::Object(&concise_arrow)).is_none()
        );

        let non_function = parse_modules_object(
            r#"
const modules = {
  1: 42
};
"#,
        );
        assert!(
            collect_module_descriptors(&Webpack5ModulesContainer::Object(&non_function)).is_none()
        );
    }

    #[test]
    fn array_container_collects_ids_from_holey_array() {
        let init = parse_first_var_init(
            r#"
const modules = ([
  ,
  function(module, exports, require) { exports.a = 1; },
  ,
  (module, exports, require) => { exports.b = 2; }
]);
"#,
        );
        let container =
            Webpack5ModulesContainer::from_expr(&init).expect("array container should match");
        let descriptors =
            collect_module_descriptors(&container).expect("descriptors should collect");
        let ids: Vec<&str> = descriptors.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "3"]);
        assert_eq!(descriptors[0].filename, "module-1.js");
    }

    #[test]
    fn array_container_applies_concat_offset_and_skips_false() {
        let init = parse_first_var_init(
            r#"
const modules = Array(4).concat([
  function(module, exports, require) { exports.a = 1; },
  false,
  function(module, exports, require) { exports.b = 2; }
]);
"#,
        );
        let container =
            Webpack5ModulesContainer::from_expr(&init).expect("concat container should match");
        let descriptors =
            collect_module_descriptors(&container).expect("descriptors should collect");
        let ids: Vec<&str> = descriptors.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["4", "6"]);
    }

    #[test]
    fn array_container_rejects_invalid_shapes() {
        // Non-function element
        let non_function = parse_first_var_init("const modules = [function(m, e, r) {}, 42];");
        assert!(Webpack5ModulesContainer::from_expr(&non_function).is_none());

        // Spread element
        let spread = parse_first_var_init("const modules = [...others];");
        assert!(Webpack5ModulesContainer::from_expr(&spread).is_none());

        // Only holes and placeholders — no factory
        let empty = parse_first_var_init("const modules = [, false];");
        assert!(Webpack5ModulesContainer::from_expr(&empty).is_none());

        // Concise arrow body is not a factory
        let concise = parse_first_var_init("const modules = [(m, e, r) => e.a];");
        assert!(Webpack5ModulesContainer::from_expr(&concise).is_none());
    }

    #[test]
    fn detects_wp5_cjs_min_numeric_keys_and_method_shorthand() {
        // Minified webpack 5 CJS bundle with numeric keys and method shorthand syntax
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-cjs-min/bundle.js"
        ))
        .expect("failed to read wp5-cjs-min fixture");

        let result = detect_and_extract(&source).expect("wp5-cjs-min should be detected");
        let module_ids: Vec<&str> = result.modules.iter().map(|m| m.id.as_str()).collect();

        // Should extract modules with numeric IDs
        assert!(
            result.modules.len() >= 2,
            "expected at least 2 modules, got {}: {:?}",
            result.modules.len(),
            module_ids
        );
        // Numeric module IDs should be present
        assert!(
            module_ids.iter().any(|id| id.parse::<i64>().is_ok()),
            "expected numeric module IDs, got {:?}",
            module_ids
        );
    }

    #[test]
    fn profiler_separates_webpack_fixer_and_emit() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-cjs-min/bundle.js"
        ))
        .expect("failed to read wp5-cjs-min fixture");

        let (result, spans) = crate::test_tracing::record_spans(|| {
            detect_and_extract(&source).expect("wp5-cjs-min should be detected")
        });

        assert!(!result.modules.is_empty());
        for expected in ["webpack5: fixer", "unpacker: prepared emit"] {
            assert!(
                spans.iter().any(|name| name == expected),
                "missing {expected:?} in {spans:?}"
            );
        }
    }

    #[test]
    fn detects_wp5_require_s_entry() {
        // Webpack 5 bundle using __webpack_require__(__webpack_require__.s = 2) for entry
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-require-s/bundle.js"
        ))
        .expect("failed to read wp5-require-s fixture");

        let result = detect_and_extract(&source).expect("wp5-require-s should be detected");
        let module_ids: Vec<&str> = result.modules.iter().map(|m| m.id.as_str()).collect();

        assert!(
            result.modules.len() >= 2,
            "expected at least 2 modules, got {}: {:?}",
            result.modules.len(),
            module_ids
        );
        // Module "2" should be marked as entry
        let entry_modules: Vec<&str> = result
            .modules
            .iter()
            .filter(|m| m.is_entry)
            .map(|m| m.id.as_str())
            .collect();
        assert!(
            entry_modules.contains(&"2"),
            "expected module '2' to be marked as entry, entries: {:?}",
            entry_modules
        );
    }

    #[test]
    fn detects_wp5_require_o_entry() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-require-o/bundle.js"
        ))
        .expect("failed to read wp5-require-o fixture");

        assert!(
            source.contains(".O(void 0") && source.contains("=>"),
            "fixture should reproduce webpack 5 require.O arrow startup:\n{source}"
        );

        let result = detect_and_extract(&source).expect("wp5 require.O startup should be detected");
        let entry_modules: Vec<&str> = result
            .modules
            .iter()
            .filter(|m| m.is_entry)
            .map(|m| m.id.as_str())
            .collect();

        assert!(
            entry_modules.len() == 1,
            "expected require.O callback target to be marked as entry"
        );
        let entry_id = entry_modules[0];
        let entry = result
            .modules
            .iter()
            .find(|module| module.id == entry_id)
            .expect("entry id should refer to an extracted module");
        assert!(
            entry.code.contains("entry:"),
            "entry marker should come from require-o-entry.js:\n{}",
            entry.code
        );
    }

    #[test]
    fn detects_ncc_inline_entry() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-ncc/index.cjs"
        ))
        .expect("failed to read generated ncc fixture");

        let result = detect_and_extract(&source).expect("ncc bundle should be detected");
        let entry = result
            .modules
            .iter()
            .find(|module| module.filename == "entry.js")
            .expect("ncc inline startup should become entry.js");

        assert!(
            entry.is_entry,
            "synthetic ncc entry should be marked as entry"
        );
        assert!(
            entry.code.contains(r#"require("./module-582.js")"#),
            "ncc runtime require should be normalized:\n{}",
            entry.code
        );
        assert!(
            !entry.code.contains("__nccwpck_require__"),
            "ncc runtime name should not survive:\n{}",
            entry.code
        );
    }

    #[test]
    fn detects_ncc_direct_commonjs_entry() {
        let mut source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-ncc/index.cjs"
        ))
        .expect("failed to read generated ncc fixture");
        let entry_start = source
            .find("var __webpack_exports__ = {};")
            .expect("fixture should contain inline entry start");
        let export = "module.exports = __webpack_exports__;";
        let entry_end = source[entry_start..]
            .find(export)
            .map(|offset| entry_start + offset + export.len())
            .expect("fixture should contain final CommonJS export");
        source.replace_range(
            entry_start..entry_end,
            "module.exports = __nccwpck_require__(582);",
        );

        let result = detect_and_extract(&source).expect("direct ncc startup should be detected");
        assert!(
            result
                .modules
                .iter()
                .any(|module| module.id == "582" && module.is_entry),
            "the directly required module should be marked as entry: {:?}",
            result
                .modules
                .iter()
                .map(|module| (&module.id, module.is_entry))
                .collect::<Vec<_>>()
        );
        assert!(
            result.modules.iter().all(|module| module.id != "entry"),
            "direct startup should not fabricate a synthetic entry"
        );
    }

    #[test]
    fn detects_ncc_export_assignment_in_sequence() {
        let mut source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-ncc/index.cjs"
        ))
        .expect("failed to read generated ncc fixture");
        source = source.replacen(
            "module.exports = __webpack_exports__;",
            "(recordStartup(), module.exports = __webpack_exports__);",
            1,
        );

        let result = detect_and_extract(&source).expect("sequenced ncc export should be detected");
        let entry = result
            .modules
            .iter()
            .find(|module| module.id == "entry")
            .expect("sequenced export should retain the synthetic entry");
        assert!(entry.is_entry);
        assert!(entry.code.contains("recordStartup()"), "{}", entry.code);
    }

    #[test]
    fn keeps_extracted_modules_when_synthetic_entry_emission_fails() {
        let mut modules = vec![UnpackedModule {
            id: "582".to_string(),
            filename: "module-582.js".to_string(),
            ..Default::default()
        }];
        let mut prepared = vec![None];

        assert!(!append_synthetic_entry(
            &mut modules,
            &mut prepared,
            Vec::new(),
            None,
        ));
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].id, "582");
        assert_eq!(prepared.len(), modules.len());
    }

    #[test]
    fn materializes_trailing_iife_entry_with_aligned_prepared_modules() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-cjs/bundle.js"
        ))
        .expect("failed to read generated webpack fixture");

        let result = detect_and_extract(&source)
            .expect("webpack bundle with a trailing IIFE entry should materialize");
        assert!(
            result
                .modules
                .iter()
                .any(|module| module.filename == "entry.js" && module.is_entry),
            "trailing IIFE should become an entry module"
        );
    }

    #[test]
    fn sanitize_filename_strips_dot_slash() {
        assert_eq!(sanitize_filename("./src/index.js"), "src/index.js");
    }

    #[test]
    fn sanitize_filename_strips_path_traversal() {
        assert_eq!(sanitize_filename("../../../etc/passwd"), "etc/passwd");
        assert_eq!(sanitize_filename("./../../foo.js"), "foo.js");
        assert_eq!(
            sanitize_filename("....//node_modules/@wakaru/cli/bin/wakaru"),
            "..../node_modules/@wakaru/cli/bin/wakaru"
        );
    }

    #[test]
    fn sanitize_filename_strips_backslash_traversal() {
        assert_eq!(
            sanitize_filename("./\\..\\node_modules\\debug\\src\\index"),
            "node_modules/debug/src/index"
        );
    }

    #[test]
    fn sanitize_filename_empty_after_strip() {
        assert_eq!(sanitize_filename("../"), "unknown.js");
        assert_eq!(sanitize_filename("./"), "unknown.js");
    }

    #[test]
    fn sanitize_filename_preserves_normal_paths() {
        assert_eq!(sanitize_filename("src/utils.js"), "src/utils.js");
        assert_eq!(sanitize_filename("index.js"), "index.js");
    }

    #[test]
    fn detects_wp5_cjs_string_keys() {
        // Non-minified webpack 5 CJS bundle with string keys (and method shorthand)
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-cjs/bundle.js"
        ))
        .expect("failed to read wp5-cjs fixture");

        let result = detect_and_extract(&source).expect("wp5-cjs should be detected");
        let module_ids: Vec<&str> = result.modules.iter().map(|m| m.id.as_str()).collect();

        // Should have 3 modules: 2 library modules + 1 entry
        assert_eq!(
            result.modules.len(),
            3,
            "expected 3 modules, got {}: {:?}",
            result.modules.len(),
            module_ids
        );
        assert!(
            result.modules.iter().any(|m| m.is_entry),
            "expected an entry module, got {:?}",
            module_ids
        );
    }
}
