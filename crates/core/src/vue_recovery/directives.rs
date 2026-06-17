use anyhow::Result;
use swc_core::ecma::ast::{Expr, Lit, Prop, PropOrSpread, UnaryOp};

use super::context::resolve_directive_name;
use super::expressions::{clean_attr_expr, print_expr};
use super::helpers::VueHelper;
use super::syntax::{prop_name, string_lit};
use super::VueRecoveryContext;
use crate::vue_template::{VueAttr, VueDirective, VueDirectiveArg, VueExpr};

pub(super) fn recover_directive_tuple(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueAttr>> {
    let Expr::Array(tuple) = expr else {
        return Ok(None);
    };
    let Some(helper_expr) = tuple.elems.first().and_then(|elem| elem.as_ref()) else {
        return Ok(None);
    };
    let Some(name) = directive_name(helper_expr.expr.as_ref(), ctx) else {
        return Ok(None);
    };
    let expr = tuple
        .elems
        .get(1)
        .and_then(|elem| elem.as_ref())
        .map(|elem| directive_expr(elem.expr.as_ref(), ctx))
        .transpose()?
        .flatten();
    let arg = tuple
        .elems
        .get(2)
        .and_then(|elem| elem.as_ref())
        .map(|elem| directive_arg(elem.expr.as_ref(), ctx))
        .transpose()?
        .flatten();
    let modifiers = tuple
        .elems
        .get(3)
        .and_then(|elem| elem.as_ref())
        .map(|elem| directive_modifiers(elem.expr.as_ref()))
        .unwrap_or_default();
    Ok(Some(VueAttr::Directive(VueDirective {
        name: name.name,
        arg: arg.map(|arg| {
            if arg.dynamic {
                VueDirectiveArg::Dynamic(VueExpr::new(arg.value))
            } else {
                VueDirectiveArg::Static(arg.value)
            }
        }),
        expr,
        modifiers,
    })))
}

pub(super) fn directive_modifiers(expr: &Expr) -> Vec<String> {
    let Expr::Object(object) = expr else {
        return Vec::new();
    };
    object
        .props
        .iter()
        .filter_map(|prop| {
            let PropOrSpread::Prop(prop) = prop else {
                return None;
            };
            match prop.as_ref() {
                Prop::KeyValue(key_value) => {
                    if matches!(key_value.value.as_ref(), Expr::Lit(Lit::Bool(bool)) if !bool.value)
                    {
                        return None;
                    }
                    prop_name(&key_value.key)
                }
                Prop::Shorthand(ident) => Some(ident.sym.to_string()),
                _ => None,
            }
        })
        .collect()
}

struct RecoveredDirectiveName {
    name: String,
}

struct RecoveredDirectiveArg {
    value: String,
    dynamic: bool,
}

fn directive_name(expr: &Expr, ctx: &VueRecoveryContext) -> Option<RecoveredDirectiveName> {
    match expr {
        Expr::Ident(ident) => {
            if let Some(helper) = ctx.vue_helpers.get(&ident.sym) {
                match helper {
                    VueHelper::VModel(_) => {
                        return Some(RecoveredDirectiveName {
                            name: "model".to_string(),
                        });
                    }
                    VueHelper::VShow => {
                        return Some(RecoveredDirectiveName {
                            name: "show".to_string(),
                        });
                    }
                    _ => {}
                }
            }
            ctx.directive_bindings
                .get(&ident.sym)
                .cloned()
                .map(|name| RecoveredDirectiveName { name })
        }
        Expr::Call(_) => {
            resolve_directive_name(expr, ctx).map(|name| RecoveredDirectiveName { name })
        }
        _ => None,
    }
}

fn directive_arg(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Option<RecoveredDirectiveArg>> {
    if is_absent_directive_value(expr) {
        return Ok(None);
    }
    if let Some(value) = string_lit(expr) {
        return Ok(Some(RecoveredDirectiveArg {
            value,
            dynamic: false,
        }));
    }
    Ok(Some(RecoveredDirectiveArg {
        value: clean_attr_expr(&print_expr(expr, ctx)?, ctx),
        dynamic: true,
    }))
}

fn directive_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Option<VueExpr>> {
    if is_absent_directive_value(expr) {
        return Ok(None);
    }
    print_expr(expr, ctx).map(|expr| Some(VueExpr::new(clean_attr_expr(&expr, ctx))))
}

fn is_absent_directive_value(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Null(_)) => true,
        Expr::Ident(ident) if ident.sym.as_ref() == "undefined" => true,
        Expr::Unary(unary) if unary.op == UnaryOp::Void => true,
        Expr::Paren(paren) => is_absent_directive_value(paren.expr.as_ref()),
        _ => false,
    }
}
