use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    Expr, Ident, MemberExpr, MemberProp, Module, ModuleItem, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::rules::constructor_sensitivity::static_member_name;
use crate::rules::expr_utils::is_unresolved_ident;
use crate::utils::paren::strip_parens;

/// Name reported for a use of the whole `exports` object.
pub(crate) const WHOLE_EXPORTS: &str = "exports";
/// Name reported for a use of the whole `module.exports` object.
pub(crate) const WHOLE_MODULE_EXPORTS: &str = "module.exports";

/// Export names that an ES module output still reads or writes through the
/// CommonJS `exports` object, in first-appearance order.
///
/// ESM has no `exports` binding, so each listed access throws a
/// `ReferenceError` when it runs. A static `exports.name`,
/// `exports["name"]`, or `module.exports.name` access reports `name`. Any
/// other use of the object (an escape, a computed key, a whole-value
/// assignment) reports [`WHOLE_EXPORTS`] or [`WHOLE_MODULE_EXPORTS`].
///
/// Output without import or export declarations is CommonJS, where these
/// accesses are valid, so it reports nothing. A direct `typeof exports` probe
/// is safe without the binding and is not reported. Other `module` members
/// such as `module.hot` are not exports and are not reported either.
///
/// `module` must be resolved with `unresolved_mark`.
pub(crate) fn unrecovered_commonjs_export_names(
    module: &Module,
    unresolved_mark: Mark,
) -> Vec<Atom> {
    if !module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return Vec::new();
    }
    let mut collector = ResidualCollector {
        unresolved_mark,
        names: Vec::new(),
    };
    module.visit_with(&mut collector);
    collector.names
}

struct ResidualCollector {
    unresolved_mark: Mark,
    names: Vec<Atom>,
}

impl ResidualCollector {
    fn record(&mut self, name: Atom) {
        if !self.names.contains(&name) {
            self.names.push(name);
        }
    }

    fn is_global(&self, ident: &Ident, name: &str) -> bool {
        is_unresolved_ident(ident, name, self.unresolved_mark)
    }

    fn is_module_exports(&self, expr: &Expr) -> bool {
        let Expr::Member(member) = strip_parens(expr) else {
            return false;
        };
        matches!(member.obj.as_ref(), Expr::Ident(object) if self.is_global(object, "module"))
            && static_member_name(&member.prop).as_deref() == Some("exports")
    }

    fn record_property(&mut self, member: &MemberExpr, whole: &str) {
        let name = static_member_name(&member.prop).unwrap_or_else(|| Atom::from(whole));
        self.record(name);
        if let MemberProp::Computed(computed) = &member.prop {
            computed.visit_with(self);
        }
    }
}

impl Visit for ResidualCollector {
    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::TypeOf
            && matches!(strip_parens(&unary.arg), Expr::Ident(ident) if self.is_global(ident, "exports"))
        {
            return;
        }
        unary.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        match strip_parens(&member.obj) {
            Expr::Ident(object) if self.is_global(object, "exports") => {
                self.record_property(member, WHOLE_EXPORTS);
            }
            Expr::Ident(object) if self.is_global(object, "module") => {
                if static_member_name(&member.prop).as_deref() == Some("exports") {
                    self.record(Atom::from(WHOLE_MODULE_EXPORTS));
                } else if let MemberProp::Computed(computed) = &member.prop {
                    computed.visit_with(self);
                }
            }
            object if self.is_module_exports(object) => {
                self.record_property(member, WHOLE_MODULE_EXPORTS);
            }
            _ => member.visit_children_with(self),
        }
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if self.is_global(ident, "exports") {
            self.record(Atom::from(WHOLE_EXPORTS));
        }
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{parse_file_as_module, EsSyntax, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;

    fn names(source: &str) -> Vec<String> {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(FileName::Anon.into(), source.to_string());
            let mut module = parse_file_as_module(
                &file,
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                None,
                &mut Vec::new(),
            )
            .expect("test source parses");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            unrecovered_commonjs_export_names(&module, unresolved_mark)
                .into_iter()
                .map(|name| name.to_string())
                .collect()
        })
    }

    #[test]
    fn reports_static_property_accesses_in_first_appearance_order() {
        assert_eq!(
            names(
                "export let a = 1;
                 function bump() { exports.count += 1; return exports.count < exports['limit']; }
                 module.exports.extra = 2;
                 typeof exports.maybe;"
            ),
            ["count", "limit", "extra", "maybe"]
        );
    }

    #[test]
    fn reports_whole_object_uses() {
        assert_eq!(
            names("export {}; register(exports); module.exports = value; exports[key] = 1;"),
            ["exports", "module.exports"]
        );
    }

    #[test]
    fn commonjs_output_reports_nothing() {
        assert!(names("exports.count = 0; function bump() { exports.count++; }").is_empty());
    }

    #[test]
    fn ignores_typeof_probes_other_module_members_and_locals() {
        assert!(names(
            "export {};
             if (typeof exports === 'object' && typeof module !== 'undefined' && module.hot) {}
             function factory(exports, module) { exports.a = module.exports; }"
        )
        .is_empty());
    }
}
