use swc_core::atoms::Wtf8Atom;
use swc_core::ecma::ast::{Expr, Ident, Lit, ModuleExportName, Param, Pat, PropName};

pub(super) fn module_export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.to_string(),
        ModuleExportName::Str(str) => wtf8_to_string(&str.value),
    }
}

pub(super) fn param_binding_ident(param: &Param) -> Option<&Ident> {
    match &param.pat {
        Pat::Ident(binding) => Some(&binding.id),
        _ => None,
    }
}

pub(super) fn prop_name(name: &PropName) -> Option<String> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(str) => Some(wtf8_to_string(&str.value)),
        _ => None,
    }
}

pub(super) fn string_lit(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(Lit::Str(str)) => Some(wtf8_to_string(&str.value)),
        Expr::Tpl(tpl) if tpl.exprs.is_empty() && tpl.quasis.len() == 1 => {
            let quasi = tpl.quasis.first()?;
            quasi
                .cooked
                .as_ref()
                .map(wtf8_to_string)
                .or_else(|| Some(quasi.raw.to_string()))
        }
        _ => None,
    }
}

pub(super) fn wtf8_to_string(value: &Wtf8Atom) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string_lossy().into_owned())
}
