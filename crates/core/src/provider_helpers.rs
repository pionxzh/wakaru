//! The interop helpers a module exports, read before `UnEsm` on the AST it
//! will see, for the plan built at barrier 0 ([`crate::interface_plan`]).
//!
//! A bundle can carry a transpiler helper library as its own module: tslib
//! under TypeScript's `importHelpers`, `@babel/runtime`, or `@swc/helpers`.
//! A consumer then reaches the helper through `require("./m.js")`, not
//! `require("tslib")` or a helper runtime path, so nothing about the module's
//! name proves what it exports. An entry here is proven on two sides:
//!
//! - the export reaches a function the module declares once and never
//!   writes, or a function literal, through a storage model the export
//!   storage analysis accepts (a getter, a mirrored binding, or a property
//!   written once), or as the whole `module.exports = helper` value;
//! - that function has the body of a default or wildcard interop helper
//!   ([`classify_interop_helper_function`]), whatever it is called. Terser's
//!   renamed `function n(e) {…}` counts; a function named `__importDefault`
//!   with another body does not.
//!
//! The export names come from webpack's `require.d` getters (the object form
//! and webpack 4's string form), `Object.defineProperty(exports, …)` getters,
//! and `exports.name = …` writes, with the `__esModule` markers set aside as
//! `UnEsm` sets them aside (`require.r(exports)` included). A
//! `module.exports = helper` provider is the value itself; Babel's runtime
//! shim also mirrors it as `module.exports.default = module.exports`, which
//! gives it a `default` member too.
//!
//! Anything else is absent: a module with ESM declarations, a reassigned
//! binding, an export with more than one storage, or a UMD build that
//! defines its exports through a callback (`exporter("__importDefault", …)`).

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, Decl, Expr, Ident, MemberExpr, Module, ModuleItem, Pat,
    SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::analysis::BindingId;
use crate::collections::{HashMap, HashSet};
use crate::facts::collect_commonjs_value_is_default;
use crate::require_edges::CalleeExport;
use crate::rules::transpiler_helper_utils::{
    classify_interop_helper_expr, classify_interop_helper_function, TranspilerHelperKind,
};
use crate::rules::{analyze_export_storage_without_markers, ExportStorage, ExportStorageReport};
use crate::utils::member::static_member_name;
use crate::utils::paren::strip_parens;

/// The interop helpers one module exports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProviderHelpers {
    /// `module.exports = helper`: the required value is the helper.
    value: Option<TranspilerHelperKind>,
    /// Properties of the required value that hold a helper, in the order
    /// the module defines them.
    named: Vec<(Atom, TranspilerHelperKind)>,
}

impl ProviderHelpers {
    /// The helpers `module` exports, or `None` when it exports none this
    /// reader can prove.
    pub(crate) fn collect(module: &Module, unresolved_mark: Mark) -> Option<Self> {
        if module
            .body
            .iter()
            .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
        {
            return None;
        }
        let mut functions = helper_functions(module);
        if !exports_a_helper(module, unresolved_mark, &functions) {
            return None;
        }
        // A binding written anywhere may no longer hold the helper when a
        // consumer calls it.
        let written = BindingUseIndex::collect_direct_write_bindings(module);
        functions.retain(|binding, _| !written.contains(binding));

        let helpers = if collect_commonjs_value_is_default(module, unresolved_mark) {
            whole_value_helpers(module, unresolved_mark, &functions)
        } else {
            named_helpers(module, unresolved_mark, &functions)
        };
        (helpers.value.is_some() || !helpers.named.is_empty()).then_some(helpers)
    }

    /// The helper a consumer reaches through `export` of the required
    /// value.
    pub(crate) fn kind_of(&self, export: &CalleeExport) -> Option<TranspilerHelperKind> {
        match export {
            CalleeExport::Value => self.value,
            CalleeExport::Named(name) => self.named(name),
            CalleeExport::Other => None,
        }
    }

    /// The helper the required value holds as the property `name`.
    pub(crate) fn named(&self, name: &str) -> Option<TranspilerHelperKind> {
        self.named
            .iter()
            .find_map(|(exported, kind)| (exported.as_ref() == name).then_some(*kind))
    }

    /// The helper the required value is.
    pub(crate) fn value(&self) -> Option<TranspilerHelperKind> {
        self.value
    }

    /// Whether some export holds a helper of `kind`.
    pub(crate) fn exports_kind(&self, kind: TranspilerHelperKind) -> bool {
        self.value == Some(kind) || self.named.iter().any(|(_, named)| *named == kind)
    }

    /// Each export, `None` naming the required value itself.
    pub(crate) fn entries(
        &self,
    ) -> impl Iterator<Item = (Option<&Atom>, TranspilerHelperKind)> + '_ {
        self.value
            .map(|kind| (None, kind))
            .into_iter()
            .chain(self.named.iter().map(|(name, kind)| (Some(name), *kind)))
    }
}

/// The module's top-level interop helper functions, each declared once: a
/// function declaration, or a declarator with a function value.
fn helper_functions(module: &Module) -> HashMap<BindingId, TranspilerHelperKind> {
    let mut functions: HashMap<BindingId, TranspilerHelperKind> = HashMap::default();
    let mut declared: HashSet<BindingId> = HashSet::default();
    let mut redeclared: HashSet<BindingId> = HashSet::default();
    let mut declare = |id: BindingId, kind: Option<TranspilerHelperKind>| {
        if !declared.insert(id.clone()) {
            redeclared.insert(id.clone());
        }
        if let Some(kind) = kind {
            functions.insert(id, kind);
        }
    };
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => declare(
                binding_id(&fn_decl.ident),
                classify_interop_helper_function(&fn_decl.function),
            ),
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for declarator in &var.decls {
                    let Pat::Ident(binding) = &declarator.name else {
                        continue;
                    };
                    declare(
                        binding_id(&binding.id),
                        declarator
                            .init
                            .as_deref()
                            .and_then(classify_interop_helper_expr),
                    );
                }
            }
            _ => {}
        }
    }
    functions.retain(|binding, _| !redeclared.contains(binding));
    functions
}

/// Whether a top-level statement that touches `module` or `exports` names a
/// helper function, or a top-level assignment to an export holds a helper
/// literal: a cheap test before the whole module is read. Most modules
/// export no helper, and many declare an interop helper only for their own
/// requires.
fn exports_a_helper(
    module: &Module,
    unresolved_mark: Mark,
    functions: &HashMap<BindingId, TranspilerHelperKind>,
) -> bool {
    struct Finder<'a> {
        unresolved_mark: Mark,
        functions: &'a HashMap<BindingId, TranspilerHelperKind>,
        commonjs: bool,
        helper: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.ctxt.outer() == self.unresolved_mark
                && matches!(ident.sym.as_ref(), "module" | "exports")
            {
                self.commonjs = true;
            } else if self.functions.contains_key(&binding_id(ident)) {
                self.helper = true;
            }
        }
    }

    let literal = top_level_assignments(module).any(|assign| {
        assign_member(assign).is_some_and(|target| {
            is_exports_object(&target.obj, unresolved_mark)
                || is_module_exports(target, unresolved_mark)
        }) && classify_interop_helper_expr(&assign.right).is_some()
    });
    if literal {
        return true;
    }
    if functions.is_empty() {
        return false;
    }
    module.body.iter().any(|item| {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            return false;
        };
        let mut finder = Finder {
            unresolved_mark,
            functions,
            commonjs: false,
            helper: false,
        };
        statement.visit_with(&mut finder);
        finder.commonjs && finder.helper
    })
}

/// A provider whose CommonJS value is its single `module.exports = v`
/// statement: `v` is the helper, and Babel's runtime shim
/// `module.exports.default = module.exports` gives it a `default` member.
fn whole_value_helpers(
    module: &Module,
    unresolved_mark: Mark,
    functions: &HashMap<BindingId, TranspilerHelperKind>,
) -> ProviderHelpers {
    let mut helpers = ProviderHelpers::default();
    let mut values = top_level_assignments(module).filter_map(|assign| {
        is_module_exports(assign_member(assign)?, unresolved_mark).then_some(&assign.right)
    });
    let (Some(value), None) = (values.next(), values.next()) else {
        return helpers;
    };
    let kind = match strip_parens(value) {
        Expr::Ident(ident) => functions.get(&binding_id(ident)).copied(),
        value => classify_interop_helper_expr(value),
    };
    let Some(kind) = kind else {
        return helpers;
    };
    helpers.value = Some(kind);

    // The mirror must be the only write of a `default` property, on
    // `module.exports` or on the helper binding.
    let binding = match strip_parens(value) {
        Expr::Ident(ident) => Some(binding_id(ident)),
        _ => None,
    };
    let mirrors = top_level_assignments(module)
        .filter(|assign| {
            assign_member(assign).is_some_and(|target| {
                is_member_named(target, "default")
                    && is_module_exports_expr(&target.obj, unresolved_mark)
            }) && is_module_exports_expr(&assign.right, unresolved_mark)
        })
        .count();
    let mut default_writes = DefaultWrites {
        unresolved_mark,
        binding: binding.as_ref(),
        count: 0,
    };
    module.visit_with(&mut default_writes);
    if mirrors == 1 && default_writes.count == 1 {
        helpers.named.push((Atom::from("default"), kind));
    }
    helpers
}

/// A provider with named exports: each name's storage, by the export
/// storage analysis of the module without its marker statements.
fn named_helpers(
    module: &Module,
    unresolved_mark: Mark,
    functions: &HashMap<BindingId, TranspilerHelperKind>,
) -> ProviderHelpers {
    let mut helpers = ProviderHelpers::default();
    let ExportStorageReport::Names(decisions) =
        analyze_export_storage_without_markers(module, unresolved_mark)
    else {
        return helpers;
    };
    for decision in decisions {
        let kind = match decision.storage {
            ExportStorage::Getter => decision
                .getter
                .as_ref()
                .and_then(|binding| functions.get(binding))
                .copied(),
            ExportStorage::Mirror => decision
                .mirror
                .as_ref()
                .and_then(|mirror| functions.get(&mirror.binding))
                .copied(),
            // One top-level write of a function literal, or of a helper
            // function, and no other.
            ExportStorage::Property
                if decision.accesses.writes == 1 && decision.accesses.other_writes == 0 =>
            {
                property_value(module, unresolved_mark, &decision.name).and_then(|value| {
                    match strip_parens(value) {
                        Expr::Ident(ident) => functions.get(&binding_id(ident)).copied(),
                        value => classify_interop_helper_expr(value),
                    }
                })
            }
            _ => None,
        };
        if let Some(kind) = kind {
            helpers.named.push((decision.name, kind));
        }
    }
    helpers
}

/// The value of the top-level `exports.name = value` statement.
fn property_value<'a>(module: &'a Module, unresolved_mark: Mark, name: &Atom) -> Option<&'a Expr> {
    top_level_assignments(module).find_map(|assign| {
        let target = assign_member(assign)?;
        (is_member_named(target, name) && is_exports_object(&target.obj, unresolved_mark))
            .then_some(assign.right.as_ref())
    })
}

/// The plain assignments that are a top-level expression statement, or an
/// element of one that is a sequence.
fn top_level_assignments(module: &Module) -> impl Iterator<Item = &AssignExpr> {
    module.body.iter().flat_map(|item| {
        let exprs: Vec<&Expr> = match item {
            ModuleItem::Stmt(Stmt::Expr(statement)) => match strip_parens(&statement.expr) {
                Expr::Seq(seq) => seq.exprs.iter().map(|expr| strip_parens(expr)).collect(),
                expr => vec![expr],
            },
            _ => Vec::new(),
        };
        exprs.into_iter().filter_map(|expr| match expr {
            Expr::Assign(assign) if assign.op == AssignOp::Assign => Some(assign),
            _ => None,
        })
    })
}

fn assign_member(assign: &AssignExpr) -> Option<&MemberExpr> {
    match &assign.left {
        AssignTarget::Simple(SimpleAssignTarget::Member(member)) => Some(member),
        _ => None,
    }
}

/// Counts the writes of a `default` property on `module.exports` or on the
/// helper binding, anywhere in the module.
struct DefaultWrites<'a> {
    unresolved_mark: Mark,
    binding: Option<&'a BindingId>,
    count: usize,
}

impl Visit for DefaultWrites<'_> {
    fn visit_simple_assign_target(&mut self, target: &SimpleAssignTarget) {
        if let SimpleAssignTarget::Member(member) = target {
            if is_member_named(member, "default")
                && (is_module_exports_expr(&member.obj, self.unresolved_mark)
                    || matches!(strip_parens(&member.obj), Expr::Ident(ident)
                        if self.binding == Some(&binding_id(ident))))
            {
                self.count += 1;
            }
        }
        target.visit_children_with(self);
    }
}

fn is_member_named(member: &MemberExpr, name: &str) -> bool {
    static_member_name(&member.prop).is_some_and(|prop| prop.as_ref() == name)
}

/// `module.exports`
fn is_module_exports(member: &MemberExpr, unresolved_mark: Mark) -> bool {
    is_member_named(member, "exports")
        && matches!(strip_parens(&member.obj), Expr::Ident(object)
            if object.sym.as_ref() == "module" && object.ctxt.outer() == unresolved_mark)
}

fn is_module_exports_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    matches!(strip_parens(expr), Expr::Member(member) if is_module_exports(member, unresolved_mark))
}

/// `exports` or `module.exports`
fn is_exports_object(expr: &Expr, unresolved_mark: Mark) -> bool {
    match strip_parens(expr) {
        Expr::Ident(ident) => {
            ident.sym.as_ref() == "exports" && ident.ctxt.outer() == unresolved_mark
        }
        expr => is_module_exports_expr(expr, unresolved_mark),
    }
}

fn binding_id(ident: &Ident) -> BindingId {
    (ident.sym.clone(), ident.ctxt)
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{parse_file_as_module, EsSyntax, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;
    use TranspilerHelperKind::{InteropRequireDefault, InteropRequireWildcard};

    fn collect(source: &str) -> Option<ProviderHelpers> {
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
            let mark = Mark::new();
            module.visit_mut_with(&mut resolver(mark, Mark::new(), false));
            ProviderHelpers::collect(&module, mark)
        })
    }

    fn named(helpers: &ProviderHelpers) -> Vec<(&str, TranspilerHelperKind)> {
        helpers
            .named
            .iter()
            .map(|(name, kind)| (name.as_ref(), *kind))
            .collect()
    }

    const IMPORT_DEFAULT: &str = r#"function __importDefault(mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
}"#;

    const IMPORT_STAR: &str = r#"var ownKeys = function(o) {
    ownKeys = Object.getOwnPropertyNames || function (o) {
        var ar = [];
        for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
        return ar;
    };
    return ownKeys(o);
};
function __importStar(mod) {
    if (mod && mod.__esModule) return mod;
    var result = {};
    if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
    __setModuleDefault(result, mod);
    return result;
}"#;

    /// producer webpack@5 bundling tslib's ES module build: tree-shaken
    /// `require.d` getters.
    #[test]
    fn webpack_getters_of_tslib() {
        let helpers = collect(&format!(
            r#""use strict";
require.d(exports, {{
    __importDefault: () => __importDefault,
    __importStar: () => __importStar,
    __extends: () => __extends,
}});
{IMPORT_DEFAULT}
{IMPORT_STAR}
function __extends(d, b) {{ d.prototype = Object.create(b.prototype); }}"#
        ))
        .expect("helpers");
        assert_eq!(
            named(&helpers),
            [
                ("__importDefault", InteropRequireDefault),
                ("__importStar", InteropRequireWildcard),
            ]
        );
        assert_eq!(helpers.value, None);
    }

    /// producer webpack@5 + terser@5: the helper renamed, the getter kept.
    #[test]
    fn terser_renamed_helper() {
        let helpers = collect(
            r#""use strict";
function n(e) {
    return e && e.__esModule ? e : { default: e };
}
require.d(exports, { __importDefault: () => n });
Object.create;"#,
        )
        .expect("helpers");
        assert_eq!(
            named(&helpers),
            [("__importDefault", InteropRequireDefault)]
        );
    }

    /// producer rspack and webpack@4: `require.r(exports)` marks the module,
    /// and webpack 4 defines each getter with the string form.
    #[test]
    fn marked_module_with_object_and_string_getters() {
        let helpers = collect(&format!(
            r#""use strict";
require.r(exports);
require.d(exports, {{ __importDefault: () => __importDefault }});
{IMPORT_DEFAULT}"#
        ))
        .expect("object form");
        assert_eq!(
            named(&helpers),
            [("__importDefault", InteropRequireDefault)]
        );

        let helpers = collect(&format!(
            r#""use strict";
require.r(exports);
require.d(exports, "__importDefault", function() {{ return __importDefault; }});
require.d(exports, "__importStar", function() {{ return __importStar; }});
{IMPORT_DEFAULT}
{IMPORT_STAR}"#
        ))
        .expect("string form");
        assert_eq!(
            named(&helpers),
            [
                ("__importDefault", InteropRequireDefault),
                ("__importStar", InteropRequireWildcard),
            ]
        );
    }

    /// producer @babel/runtime@7 helper module: the value, mirrored as its
    /// own `default`.
    #[test]
    fn babel_runtime_module_exports_with_default_mirror() {
        let helpers = collect(
            r#"function _interopRequireDefault(e) {
    return e && e.__esModule ? e : { default: e };
}
module.exports = _interopRequireDefault;
module.exports.__esModule = true;
module.exports.default = module.exports;"#,
        )
        .expect("declared");
        assert_eq!(helpers.value, Some(InteropRequireDefault));
        assert_eq!(named(&helpers), [("default", InteropRequireDefault)]);

        // Minified: the function literal is the value.
        let helpers = collect(
            r#"module.exports = function(e) {
    return e && e.__esModule ? e : { default: e };
}, module.exports.__esModule = true, module.exports.default = module.exports;"#,
        )
        .expect("minified");
        assert_eq!(helpers.value, Some(InteropRequireDefault));
        assert_eq!(named(&helpers), [("default", InteropRequireDefault)]);

        // Without the mirror, `default` is not the helper.
        let helpers = collect(
            r#"module.exports = function(e) {
    return e && e.__esModule ? e : { default: e };
};"#,
        )
        .expect("no mirror");
        assert_eq!(helpers.value, Some(InteropRequireDefault));
        assert!(helpers.named.is_empty());
    }

    /// producer @swc/helpers bundled by webpack@5: every helper module
    /// exports its callable as `_`.
    #[test]
    fn swc_helpers_underscore_export() {
        let helpers = collect(
            r#""use strict";
require.d(exports, { _: () => _interop_require_default });
function _interop_require_default(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
}"#,
        )
        .expect("helpers");
        assert_eq!(named(&helpers), [("_", InteropRequireDefault)]);
    }

    #[test]
    fn a_property_written_once_with_a_helper_literal() {
        let helpers = collect(
            r#"exports.__esModule = true;
exports.default = function (obj) {
    return obj && obj.__esModule ? obj : { default: obj };
};"#,
        )
        .expect("helpers");
        assert_eq!(named(&helpers), [("default", InteropRequireDefault)]);
    }

    #[test]
    fn a_helper_name_with_another_body_is_not_a_helper() {
        assert_eq!(
            collect(
                r#""use strict";
require.d(exports, { __importDefault: () => __importDefault });
function __importDefault(mod) {
    return mod;
}"#
            ),
            None
        );
    }

    #[test]
    fn a_reassigned_helper_or_export_is_not_proven() {
        // The binding is written after the module exports it.
        assert_eq!(
            collect(&format!(
                r#""use strict";
require.d(exports, {{ __importDefault: () => __importDefault }});
{IMPORT_DEFAULT}
__importDefault = function (mod) {{ return mod; }};"#
            )),
            None
        );
        // The property has a second storage.
        assert_eq!(
            collect(&format!(
                r#""use strict";
Object.defineProperty(exports, "__importDefault", {{ enumerable: true, get: function () {{ return __importDefault; }} }});
exports.__importDefault = other;
{IMPORT_DEFAULT}"#
            )),
            None
        );
        // `module.exports` replaced twice.
        assert_eq!(
            collect(&format!(
                r#"{IMPORT_DEFAULT}
module.exports = __importDefault;
if (x) module.exports = other;"#
            )),
            None
        );
    }

    #[test]
    fn a_module_that_only_uses_its_own_helper_exports_none() {
        assert_eq!(
            collect(&format!(
                r#""use strict";
{IMPORT_DEFAULT}
exports.run = function () {{ return __importDefault(require("./widget.js")).default; }};"#
            )),
            None
        );
    }

    /// A UMD build defines its exports through a callback, which no storage
    /// model reads.
    #[test]
    fn a_umd_exporter_callback_is_not_read() {
        assert_eq!(
            collect(
                r#"var __importDefault;
(function (factory) {
    factory(function (id, v) { return exports[id] = v; });
})(function (exporter) {
    __importDefault = function (mod) {
        return (mod && mod.__esModule) ? mod : { "default": mod };
    };
    exporter("__importDefault", __importDefault);
});"#
            ),
            None
        );
    }
}
