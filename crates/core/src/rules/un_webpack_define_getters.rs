use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    Bool, CallExpr, Callee, Decl, Expr, ExprOrSpread, ExprStmt, Ident, IdentName, KeyValueProp,
    Lit, Module, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread, Stmt, Str,
    VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

type BindingId = (Atom, SyntaxContext);

pub struct UnWebpackDefineGetters {
    unresolved_mark: Mark,
}

impl UnWebpackDefineGetters {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnWebpackDefineGetters {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(self);
        rewrite_module_items(&mut module.body, self.unresolved_mark);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        rewrite_stmts(stmts, self.unresolved_mark);
    }
}

fn rewrite_module_items(items: &mut Vec<ModuleItem>, unresolved_mark: Mark) {
    let original = std::mem::take(items);
    let mut rewritten = Vec::with_capacity(original.len());
    let mut i = 0;

    while i < original.len() {
        if let Some(binding) = extract_empty_object_binding_from_module_item(&original[i]) {
            let (replacement, next_index) =
                maybe_build_define_properties_item(&original, i + 1, &binding, unresolved_mark);
            if let Some(item) = replacement {
                rewritten.push(original[i].clone());
                rewritten.push(item);
                i = next_index;
                continue;
            }
        }

        rewritten.push(original[i].clone());
        i += 1;
    }

    *items = rewritten;
}

fn rewrite_stmts(stmts: &mut Vec<Stmt>, unresolved_mark: Mark) {
    let original = std::mem::take(stmts);
    let mut rewritten = Vec::with_capacity(original.len());
    let mut i = 0;

    while i < original.len() {
        if let Some(binding) = extract_empty_object_binding_from_stmt(&original[i]) {
            let (replacement, next_index) =
                maybe_build_define_properties_stmt(&original, i + 1, &binding, unresolved_mark);
            if let Some(stmt) = replacement {
                rewritten.push(original[i].clone());
                rewritten.push(stmt);
                i = next_index;
                continue;
            }
        }

        rewritten.push(original[i].clone());
        i += 1;
    }

    *stmts = rewritten;
}

fn maybe_build_define_properties_item(
    items: &[ModuleItem],
    start: usize,
    target: &BindingId,
    unresolved_mark: Mark,
) -> (Option<ModuleItem>, usize) {
    let (descriptors, next_index) =
        collect_require_d_descriptors_module(items, start, target, unresolved_mark);
    if descriptors.len() < 2 {
        return (None, start);
    }

    (
        Some(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(build_define_properties_call(target.clone(), descriptors)),
        }))),
        next_index,
    )
}

fn maybe_build_define_properties_stmt(
    stmts: &[Stmt],
    start: usize,
    target: &BindingId,
    unresolved_mark: Mark,
) -> (Option<Stmt>, usize) {
    let (descriptors, next_index) =
        collect_require_d_descriptors_stmt(stmts, start, target, unresolved_mark);
    if descriptors.len() < 2 {
        return (None, start);
    }

    (
        Some(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(build_define_properties_call(target.clone(), descriptors)),
        })),
        next_index,
    )
}

fn collect_require_d_descriptors_module(
    items: &[ModuleItem],
    start: usize,
    target: &BindingId,
    unresolved_mark: Mark,
) -> (Vec<(String, Box<Expr>)>, usize) {
    let mut descriptors = Vec::new();
    let mut seen = HashSet::new();
    let mut index = start;

    while index < items.len() {
        let ModuleItem::Stmt(stmt) = &items[index] else {
            break;
        };
        let Some((name, getter)) = extract_require_d_descriptor(stmt, target, unresolved_mark)
        else {
            break;
        };
        if !seen.insert(name.clone()) {
            return (Vec::new(), start);
        }
        descriptors.push((name, getter));
        index += 1;
    }

    (descriptors, index)
}

fn collect_require_d_descriptors_stmt(
    stmts: &[Stmt],
    start: usize,
    target: &BindingId,
    unresolved_mark: Mark,
) -> (Vec<(String, Box<Expr>)>, usize) {
    let mut descriptors = Vec::new();
    let mut seen = HashSet::new();
    let mut index = start;

    while index < stmts.len() {
        let Some((name, getter)) =
            extract_require_d_descriptor(&stmts[index], target, unresolved_mark)
        else {
            break;
        };
        if !seen.insert(name.clone()) {
            return (Vec::new(), start);
        }
        descriptors.push((name, getter));
        index += 1;
    }

    (descriptors, index)
}

fn extract_empty_object_binding_from_module_item(item: &ModuleItem) -> Option<BindingId> {
    match item {
        ModuleItem::Stmt(stmt) => extract_empty_object_binding_from_stmt(stmt),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
            let Decl::Var(var_decl) = &export_decl.decl else {
                return None;
            };
            extract_empty_object_binding_from_var_decl(&var_decl.decls)
        }
        _ => None,
    }
}

fn extract_empty_object_binding_from_stmt(stmt: &Stmt) -> Option<BindingId> {
    let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
        return None;
    };
    extract_empty_object_binding_from_var_decl(&var_decl.decls)
}

fn extract_empty_object_binding_from_var_decl(decls: &[VarDeclarator]) -> Option<BindingId> {
    if decls.len() != 1 {
        return None;
    }
    let decl = &decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let Expr::Object(obj) = decl.init.as_deref()? else {
        return None;
    };
    if !obj.props.is_empty() {
        return None;
    }
    Some((binding.id.sym.clone(), binding.id.ctxt))
}

fn extract_require_d_descriptor(
    stmt: &Stmt,
    target: &BindingId,
    unresolved_mark: Mark,
) -> Option<(String, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee_expr.as_ref() else {
        return None;
    };
    let Expr::Ident(require_ident) = member.obj.as_ref() else {
        return None;
    };
    if require_ident.sym.as_ref() != "require" || require_ident.ctxt.outer() != unresolved_mark {
        return None;
    }
    let swc_core::ecma::ast::MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym.as_ref() != "d" || call.args.len() != 3 {
        return None;
    }

    let Expr::Ident(target_ident) = call.args[0].expr.as_ref() else {
        return None;
    };
    if target_ident.sym != target.0 || target_ident.ctxt != target.1 {
        return None;
    }

    let Expr::Lit(Lit::Str(name)) = call.args[1].expr.as_ref() else {
        return None;
    };
    let prop_name = name.value.as_str()?.to_string();

    match call.args[2].expr.as_ref() {
        Expr::Fn(_) | Expr::Arrow(_) => Some((prop_name, call.args[2].expr.clone())),
        _ => None,
    }
}

fn build_define_properties_call(target: BindingId, descriptors: Vec<(String, Box<Expr>)>) -> Expr {
    let descriptor_props: Vec<PropOrSpread> = descriptors
        .into_iter()
        .map(|(name, getter)| {
            PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                key: make_prop_name(&name),
                value: Box::new(Expr::Object(ObjectLit {
                    span: DUMMY_SP,
                    props: vec![
                        PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                            key: PropName::Ident(IdentName::new("enumerable".into(), DUMMY_SP)),
                            value: Box::new(Expr::Lit(Lit::Bool(Bool {
                                span: DUMMY_SP,
                                value: true,
                            }))),
                        }))),
                        PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                            key: PropName::Ident(IdentName::new("get".into(), DUMMY_SP)),
                            value: getter,
                        }))),
                    ],
                })),
            })))
        })
        .collect();

    Expr::Call(CallExpr {
        span: DUMMY_SP,
        ctxt: Default::default(),
        callee: Callee::Expr(Box::new(Expr::Member(swc_core::ecma::ast::MemberExpr {
            span: DUMMY_SP,
            obj: Box::new(Expr::Ident(Ident::new_no_ctxt("Object".into(), DUMMY_SP))),
            prop: swc_core::ecma::ast::MemberProp::Ident(IdentName::new(
                "defineProperties".into(),
                DUMMY_SP,
            )),
        }))),
        args: vec![
            ExprOrSpread {
                spread: None,
                expr: Box::new(Expr::Ident(Ident::new(target.0, DUMMY_SP, target.1))),
            },
            ExprOrSpread {
                spread: None,
                expr: Box::new(Expr::Object(ObjectLit {
                    span: DUMMY_SP,
                    props: descriptor_props,
                })),
            },
        ],
        type_args: None,
    })
}

fn make_prop_name(name: &str) -> PropName {
    if is_valid_js_ident(name) {
        PropName::Ident(IdentName::new(name.into(), DUMMY_SP))
    } else {
        PropName::Str(Str {
            span: DUMMY_SP,
            value: name.into(),
            raw: None,
        })
    }
}

fn is_valid_js_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}
