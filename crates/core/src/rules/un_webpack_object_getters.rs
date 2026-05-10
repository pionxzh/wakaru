use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BlockStmt, BlockStmtOrExpr, Bool, Callee, Decl, Expr, ExprStmt, FnExpr, GetterProp, Lit,
    MemberProp, Module, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread,
    ReturnStmt, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

type BindingId = (Atom, SyntaxContext);

pub struct UnWebpackObjectGetters;

impl VisitMut for UnWebpackObjectGetters {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(self);
        rewrite_module_items(&mut module.body);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        rewrite_stmts(stmts);
    }
}

fn rewrite_module_items(items: &mut Vec<ModuleItem>) {
    let original = std::mem::take(items);
    let mut rewritten = Vec::with_capacity(original.len());
    let mut i = 0;

    while i < original.len() {
        if i + 1 < original.len() {
            if let Some(item) = maybe_rewrite_module_item_pair(&original[i], &original[i + 1]) {
                rewritten.push(item);
                i += 2;
                continue;
            }
        }

        rewritten.push(original[i].clone());
        i += 1;
    }

    *items = rewritten;
}

fn rewrite_stmts(stmts: &mut Vec<Stmt>) {
    let original = std::mem::take(stmts);
    let mut rewritten = Vec::with_capacity(original.len());
    let mut i = 0;

    while i < original.len() {
        if i + 1 < original.len() {
            if let Some(stmt) = maybe_rewrite_stmt_pair(&original[i], &original[i + 1]) {
                rewritten.push(stmt);
                i += 2;
                continue;
            }
        }

        rewritten.push(original[i].clone());
        i += 1;
    }

    *stmts = rewritten;
}

fn maybe_rewrite_module_item_pair(current: &ModuleItem, next: &ModuleItem) -> Option<ModuleItem> {
    let binding = extract_empty_object_binding_from_module_item(current)?;
    let ModuleItem::Stmt(next_stmt) = next else {
        return None;
    };
    let getters = extract_define_properties_getters(next_stmt, &binding)?;
    if getters.len() < 2 {
        return None;
    }

    match current {
        ModuleItem::Stmt(stmt) => Some(ModuleItem::Stmt(rewrite_stmt_init_with_getters(
            stmt.clone(),
            getters,
        )?)),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
            let mut export_decl = export_decl.clone();
            let Decl::Var(var_decl) = &mut export_decl.decl else {
                return None;
            };
            replace_var_decl_init_with_getters(&mut var_decl.decls, getters)?;
            Some(ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)))
        }
        _ => None,
    }
}

fn maybe_rewrite_stmt_pair(current: &Stmt, next: &Stmt) -> Option<Stmt> {
    let binding = extract_empty_object_binding_from_stmt(current)?;
    let getters = extract_define_properties_getters(next, &binding)?;
    if getters.len() < 2 {
        return None;
    }

    rewrite_stmt_init_with_getters(current.clone(), getters)
}

fn rewrite_stmt_init_with_getters(stmt: Stmt, getters: Vec<GetterProp>) -> Option<Stmt> {
    let Stmt::Decl(Decl::Var(mut var_decl)) = stmt else {
        return None;
    };
    replace_var_decl_init_with_getters(&mut var_decl.decls, getters)?;
    Some(Stmt::Decl(Decl::Var(var_decl)))
}

fn replace_var_decl_init_with_getters(
    decls: &mut [VarDeclarator],
    getters: Vec<GetterProp>,
) -> Option<()> {
    if decls.len() != 1 {
        return None;
    }
    let decl = &mut decls[0];
    let Pat::Ident(_) = &decl.name else {
        return None;
    };

    decl.init = Some(Box::new(Expr::Object(ObjectLit {
        span: DUMMY_SP,
        props: getters
            .into_iter()
            .map(|getter| PropOrSpread::Prop(Box::new(Prop::Getter(getter))))
            .collect(),
    })));

    Some(())
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

fn extract_define_properties_getters(stmt: &Stmt, target: &BindingId) -> Option<Vec<GetterProp>> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let Expr::Ident(object_ident) = member.obj.as_ref() else {
        return None;
    };
    if object_ident.sym.as_ref() != "Object" {
        return None;
    }
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym.as_ref() != "defineProperties" || call.args.len() != 2 {
        return None;
    }

    let Expr::Ident(target_ident) = call.args[0].expr.as_ref() else {
        return None;
    };
    if target_ident.sym != target.0 || target_ident.ctxt != target.1 {
        return None;
    }

    let Expr::Object(descriptor_map) = call.args[1].expr.as_ref() else {
        return None;
    };

    let mut getters = Vec::with_capacity(descriptor_map.props.len());
    for prop in &descriptor_map.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(entry) = prop.as_ref() else {
            return None;
        };
        getters.push(extract_getter_descriptor(&entry.key, entry.value.as_ref())?);
    }

    Some(getters)
}

fn extract_getter_descriptor(key: &PropName, descriptor: &Expr) -> Option<GetterProp> {
    let Expr::Object(object) = descriptor else {
        return None;
    };

    let mut enumerable_true = false;
    let mut getter_body = None;

    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(entry) = prop.as_ref() else {
            return None;
        };

        match prop_name_as_str(&entry.key)? {
            "enumerable" => {
                let Expr::Lit(Lit::Bool(Bool { value: true, .. })) = entry.value.as_ref() else {
                    return None;
                };
                enumerable_true = true;
            }
            "get" => {
                getter_body = Some(extract_getter_body(entry.value.as_ref())?);
            }
            _ => return None,
        }
    }

    if !enumerable_true {
        return None;
    }

    Some(GetterProp {
        span: DUMMY_SP,
        key: key.clone(),
        type_ann: None,
        body: Some(getter_body?),
    })
}

fn extract_getter_body(expr: &Expr) -> Option<BlockStmt> {
    match expr {
        Expr::Fn(FnExpr { ident, function }) => {
            if ident.is_some()
                || !function.params.is_empty()
                || function.is_async
                || function.is_generator
            {
                return None;
            }
            function.body.clone()
        }
        Expr::Arrow(arrow) => {
            if !arrow.params.is_empty() || arrow.is_async || arrow.is_generator {
                return None;
            }
            match arrow.body.as_ref() {
                BlockStmtOrExpr::BlockStmt(block) => Some(block.clone()),
                BlockStmtOrExpr::Expr(expr) => Some(BlockStmt {
                    span: DUMMY_SP,
                    ctxt: arrow.ctxt,
                    stmts: vec![Stmt::Return(ReturnStmt {
                        span: DUMMY_SP,
                        arg: Some(expr.clone()),
                    })],
                }),
            }
        }
        _ => None,
    }
}

fn prop_name_as_str(name: &PropName) -> Option<&str> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.as_ref()),
        PropName::Str(value) => value.value.as_str(),
        _ => None,
    }
}
