use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BindingIdent, BlockStmt, CallExpr, Callee, Decl, Expr, FnExpr, Function, Ident, Module,
    ModuleItem, Param, Pat, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::to_valid_identifier_name;

use super::eval_utils::is_direct_eval_call;
use super::rename_utils::BindingId;
use super::RewriteLevel;

pub type ExtractedFunctionNames = HashMap<BindingId, Atom>;
pub type SharedExtractedFunctionNames = Rc<RefCell<ExtractedFunctionNames>>;

pub struct ExtractInlinedFunction {
    level: RewriteLevel,
    extracted_function_names: SharedExtractedFunctionNames,
}

impl ExtractInlinedFunction {
    pub fn new(level: RewriteLevel) -> Self {
        Self::new_with_extracted_function_names(level, Default::default())
    }

    pub fn new_with_extracted_function_names(
        level: RewriteLevel,
        extracted_function_names: SharedExtractedFunctionNames,
    ) -> Self {
        Self {
            level,
            extracted_function_names,
        }
    }
}

impl VisitMut for ExtractInlinedFunction {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(self);

        if self.level < RewriteLevel::Aggressive {
            return;
        }
        if has_direct_eval_in_scope(module) {
            return;
        }

        let mut names = collect_module_binding_names(module);
        let mut extracted_function_names = self.extracted_function_names.borrow_mut();
        let mut body = Vec::with_capacity(module.body.len());
        for item in std::mem::take(&mut module.body) {
            match extract_from_module_item(item, &mut names, &mut extracted_function_names) {
                Ok((helper, item)) => {
                    body.push(ModuleItem::Stmt(helper));
                    body.push(item);
                }
                Err(item) => body.push(item),
            }
        }
        module.body = body;
    }

    fn visit_mut_block_stmt(&mut self, block: &mut BlockStmt) {
        block.visit_mut_children_with(self);

        if self.level < RewriteLevel::Aggressive {
            return;
        }
        if has_direct_eval_in_scope(block) {
            return;
        }

        let mut names = collect_stmt_binding_names(&block.stmts);
        let mut extracted_function_names = self.extracted_function_names.borrow_mut();
        let mut stmts = Vec::with_capacity(block.stmts.len());
        for stmt in std::mem::take(&mut block.stmts) {
            match extract_from_stmt(stmt, &mut names, &mut extracted_function_names) {
                Ok((helper, stmt)) => {
                    stmts.push(helper);
                    stmts.push(stmt);
                }
                Err(stmt) => stmts.push(stmt),
            }
        }
        block.stmts = stmts;
    }
}

impl Default for ExtractInlinedFunction {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

fn extract_from_module_item(
    item: ModuleItem,
    names: &mut HashSet<Atom>,
    extracted_function_names: &mut ExtractedFunctionNames,
) -> Result<(Stmt, ModuleItem), ModuleItem> {
    let ModuleItem::Stmt(stmt) = item else {
        return Err(item);
    };
    extract_from_stmt(stmt, names, extracted_function_names)
        .map(|(helper, stmt)| (helper, ModuleItem::Stmt(stmt)))
        .map_err(ModuleItem::Stmt)
}

fn extract_from_stmt(
    stmt: Stmt,
    names: &mut HashSet<Atom>,
    extracted_function_names: &mut ExtractedFunctionNames,
) -> Result<(Stmt, Stmt), Stmt> {
    let Stmt::Decl(Decl::Var(mut var)) = stmt else {
        return Err(stmt);
    };
    if var.decls.len() != 1 {
        return Err(Stmt::Decl(Decl::Var(var)));
    }

    let decl = var.decls.get_mut(0).expect("checked len == 1");
    let Some(target_name) = target_name_from_pat(&decl.name) else {
        return Err(Stmt::Decl(Decl::Var(var)));
    };
    let Some(init) = decl.init.as_mut() else {
        return Err(Stmt::Decl(Decl::Var(var)));
    };
    let Some(extraction) = extract_iife(init.as_ref(), &target_name, names) else {
        return Err(Stmt::Decl(Decl::Var(var)));
    };
    **init = extraction.call;
    extracted_function_names.insert(extraction.binding_id, Atom::from(target_name.as_str()));

    Ok((extraction.helper_stmt, Stmt::Decl(Decl::Var(var))))
}

struct Extraction {
    binding_id: BindingId,
    helper_stmt: Stmt,
    call: Expr,
}

fn extract_iife(expr: &Expr, target_name: &str, names: &mut HashSet<Atom>) -> Option<Extraction> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let function = function_from_callee(&call.callee)?;
    if !is_extractable_function(&function) {
        return None;
    }

    let param_count = function_param_count(&function);
    if param_count != call.args.len() {
        return None;
    }

    let helper_name = fresh_helper_name(target_name, names);
    names.insert(helper_name.clone());

    let helper_ident = Ident::new_no_ctxt(helper_name, DUMMY_SP);
    let binding_id = (helper_ident.sym.clone(), helper_ident.ctxt);
    let helper_expr = function.into_expr();
    let helper_stmt = const_decl_stmt(helper_ident.clone(), helper_expr);
    let call = Expr::Call(CallExpr {
        callee: Callee::Expr(Box::new(Expr::Ident(helper_ident))),
        args: call.args.clone(),
        ..call.clone()
    });

    Some(Extraction {
        binding_id,
        helper_stmt,
        call,
    })
}

enum ExtractableFunction {
    Function(Box<FnExpr>),
    Arrow(Box<swc_core::ecma::ast::ArrowExpr>),
}

impl ExtractableFunction {
    fn into_expr(self) -> Box<Expr> {
        match self {
            ExtractableFunction::Function(function) => Box::new(Expr::Fn(*function)),
            ExtractableFunction::Arrow(arrow) => Box::new(Expr::Arrow(*arrow)),
        }
    }
}

fn function_from_callee(callee: &Callee) -> Option<ExtractableFunction> {
    let Callee::Expr(expr) = callee else {
        return None;
    };
    function_from_expr(expr.as_ref())
}

fn function_from_expr(expr: &Expr) -> Option<ExtractableFunction> {
    match expr {
        Expr::Fn(function) => Some(ExtractableFunction::Function(Box::new(function.clone()))),
        Expr::Arrow(arrow) => Some(ExtractableFunction::Arrow(Box::new(arrow.clone()))),
        Expr::Paren(paren) => function_from_expr(paren.expr.as_ref()),
        _ => None,
    }
}

fn function_param_count(function: &ExtractableFunction) -> usize {
    match function {
        ExtractableFunction::Function(function) => function.function.params.len(),
        ExtractableFunction::Arrow(arrow) => arrow.params.len(),
    }
}

fn is_extractable_function(function: &ExtractableFunction) -> bool {
    match function {
        ExtractableFunction::Function(function) => {
            function.ident.is_none()
                && has_simple_function_params(&function.function)
                && function.function.body.is_some()
                && !contains_rejected_construct(&function.function)
        }
        ExtractableFunction::Arrow(arrow) => {
            has_simple_arrow_params(&arrow.params) && !contains_rejected_construct(arrow.as_ref())
        }
    }
}

fn has_simple_function_params(function: &Function) -> bool {
    function
        .params
        .iter()
        .all(|Param { pat, .. }| matches!(pat, Pat::Ident(_)))
}

fn has_simple_arrow_params(params: &[Pat]) -> bool {
    params.iter().all(|pat| matches!(pat, Pat::Ident(_)))
}

fn contains_rejected_construct<N>(node: &N) -> bool
where
    N: VisitWith<RejectedConstructFinder>,
{
    let mut finder = RejectedConstructFinder::default();
    node.visit_with(&mut finder);
    finder.found
}

#[derive(Default)]
struct RejectedConstructFinder {
    found: bool,
    function_depth: usize,
}

impl Visit for RejectedConstructFinder {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_direct_eval_call(call) {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        if self.function_depth > 0 {
            return;
        }
        self.function_depth += 1;
        function.visit_children_with(self);
        self.function_depth -= 1;
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        if self.function_depth > 0 {
            return;
        }
        self.function_depth += 1;
        arrow.visit_children_with(self);
        self.function_depth -= 1;
    }

    fn visit_this_expr(&mut self, _: &swc_core::ecma::ast::ThisExpr) {
        self.found = true;
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == *"arguments" {
            self.found = true;
        }
    }
}

fn has_direct_eval_in_scope<N>(node: &N) -> bool
where
    N: VisitWith<DirectEvalInScopeFinder> + ?Sized,
{
    let mut finder = DirectEvalInScopeFinder::default();
    node.visit_with(&mut finder);
    finder.found
}

#[derive(Default)]
struct DirectEvalInScopeFinder {
    found: bool,
}

impl Visit for DirectEvalInScopeFinder {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_direct_eval_call(call) {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }
}

fn const_decl_stmt(ident: Ident, init: Box<Expr>) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: ident,
                type_ann: None,
            }),
            init: Some(init),
            definite: false,
        }],
    })))
}

fn target_name_from_pat(pat: &Pat) -> Option<String> {
    match pat {
        Pat::Ident(binding) => Some(binding.id.sym.to_string()),
        Pat::Object(object) => object.props.iter().find_map(|prop| match prop {
            swc_core::ecma::ast::ObjectPatProp::KeyValue(key_value) => match &key_value.key {
                swc_core::ecma::ast::PropName::Ident(ident) => Some(ident.sym.to_string()),
                swc_core::ecma::ast::PropName::Str(value) => {
                    value.value.as_str().map(str::to_string)
                }
                _ => None,
            },
            swc_core::ecma::ast::ObjectPatProp::Assign(assign) => Some(assign.key.sym.to_string()),
            swc_core::ecma::ast::ObjectPatProp::Rest(_) => None,
        }),
        _ => None,
    }
}

fn fresh_helper_name(target_name: &str, names: &HashSet<Atom>) -> Atom {
    let base = format!("compute{}", pascal_case(target_name));
    let base = to_valid_identifier_name(&base);
    let base_atom = Atom::from(base.as_str());
    if !names.contains(&base_atom) {
        return base_atom;
    }

    for i in 1.. {
        let candidate = format!("{base}_{i}");
        let candidate_atom = Atom::from(candidate.as_str());
        if !names.contains(&candidate_atom) {
            return candidate_atom;
        }
    }

    unreachable!("fresh name search should not exhaust usize")
}

fn pascal_case(name: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = true;
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            capitalize_next = true;
            continue;
        }
        if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    if result.is_empty() {
        "Value".to_string()
    } else {
        result
    }
}

fn collect_module_binding_names(module: &Module) -> HashSet<Atom> {
    collect_names_with_visitor(module)
}

fn collect_stmt_binding_names(stmts: &[Stmt]) -> HashSet<Atom> {
    collect_names_with_visitor(stmts)
}

fn collect_names_with_visitor<N>(node: &N) -> HashSet<Atom>
where
    N: VisitWith<NameCollector> + ?Sized,
{
    let mut collector = NameCollector::default();
    node.visit_with(&mut collector);
    collector.names
}

#[derive(Default)]
struct NameCollector {
    names: HashSet<Atom>,
}

impl Visit for NameCollector {
    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.names.insert(ident.id.sym.clone());
    }

    fn visit_ident(&mut self, ident: &Ident) {
        self.names.insert(ident.sym.clone());
    }
}
