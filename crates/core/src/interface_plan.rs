//! The plan unpack builds at barrier 0, where every module has reached
//! `UnEsm` but none has run it, from the require edges of all modules.
//!
//! Requiring a CommonJS module returns its `exports` object, and a consumer
//! may write to it. An ES module has no such object: its namespace is not
//! extensible and its properties cannot be assigned from outside. So a
//! provider that a consumer in the input writes to stays CommonJS, and
//! `UnEsm` reads that decision here instead of guessing alone.
//!
//! A provider counts as written when an edge to it has a write in its uses
//! ([`RequireUses::writes`]): a member assignment, update, or `delete`, the
//! target of an `Object` method that writes, or the argument of a function
//! of the consumer's module that writes that parameter. A value passed to a
//! callee in another module or to an unknown one does not count; a provider
//! recovered as a namespace that meets one gets the `commonjs_exports_escape`
//! warning after the fact barrier instead.
//!
//! One written provider may still become ESM: one whose only export is a
//! single top-level `module.exports = v` ([`exports_only_whole_value`]),
//! when every write lands on `v` itself. Its ES form is `export default v`.
//! A consumer that becomes ESM writes through its default import, which
//! holds `v`; one that stays CommonJS reads `require(p).default` after the
//! fact barrier (`run_kept_require_default_reads`), which is `v` too. A
//! write counts as landing on `v` when the consumer is CommonJS in the
//! input (an ES module or webpack harmony consumer writes through a
//! namespace import), reads the value without an interop helper or an
//! `__esModule` test, and names neither `default` nor `__esModule`
//! ([`RequireUses::interop_key_write`]). The decision reads only what both
//! sides show before `UnEsm`, never the format a consumer ends up in. Such a
//! provider still counts as written: it keeps no proven helpers, and if
//! `UnEsm` leaves it CommonJS for another reason, the late ESM recovery
//! keeps it so (`keep_commonjs` in the rule pipeline options), so the facts
//! its consumers read match its output.
//!
//! A provider whose required value was never a writable CommonJS `exports`
//! object is left alone: a webpack harmony module (its exports are getters
//! `require.d` defined), and a module the input already wrote as ESM.
//!
//! The plan also carries the interop helpers each provider exports
//! ([`ProviderHelpers`]), so that `UnEsm` proves a helper a consumer reaches
//! through `require("./m.js")` as it proves one reached through
//! `require("tslib")`, but per proven export instead of by name. A written
//! provider's helpers are dropped: a consumer may have replaced one.

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{Expr, Ident, MemberExpr, MemberProp, Module};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::facts::{canonical_module_key, resolve_module_key};
use crate::facts::{collect_commonjs_value_is_default, collect_marks_es_module};
use crate::provider_helpers::ProviderHelpers;
use crate::require_edges::{CalleeExport, Interop, RequireEdge, RequireEdges, RequireUses};
use crate::rules::expr_utils::is_unresolved_ident;
use crate::rules::transpiler_helper_utils::TranspilerHelperKind;
use crate::utils::paren::strip_parens;

/// One module's input to the plan.
pub(crate) struct PlanModule<'a> {
    pub(crate) filename: &'a str,
    pub(crate) edges: &'a RequireEdges,
    /// Requiring the module returned its CommonJS `exports` object: the
    /// module has no import or export declaration and is not a webpack
    /// harmony module.
    pub(crate) exports_object_writable: bool,
    /// The interop helpers the module exports, read before `UnEsm`.
    pub(crate) helpers: Option<&'a ProviderHelpers>,
    /// The module's only export is one top-level `module.exports = v`
    /// ([`exports_only_whole_value`]).
    pub(crate) exports_whole_value: bool,
}

/// Whether the module's only export is the value of one top-level
/// `module.exports = v`, read on the input before any rule: the statement
/// runs on every evaluation of the body and is the module's only
/// `module.exports` assignment ([`collect_commonjs_value_is_default`]), no
/// other `module.exports` member is read or written, the free `exports` is
/// never referenced (no `exports.x` write, no `exports` passed to a
/// function), and the module does not mark itself `__esModule`. `UnEsm`
/// turns such a module into `export default v` alone.
pub(crate) fn exports_only_whole_value(module: &Module, unresolved_mark: Mark) -> bool {
    struct Surface {
        unresolved_mark: Mark,
        module_exports: usize,
        exports: usize,
    }
    impl Visit for Surface {
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if matches!(strip_parens(&member.obj), Expr::Ident(object)
                if is_unresolved_ident(object, "module", self.unresolved_mark))
                && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "exports")
            {
                self.module_exports += 1;
            }
            member.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &Ident) {
            if is_unresolved_ident(ident, "exports", self.unresolved_mark) {
                self.exports += 1;
            }
        }
    }
    if !collect_commonjs_value_is_default(module, unresolved_mark)
        || collect_marks_es_module(module, unresolved_mark)
    {
        return false;
    }
    let mut surface = Surface {
        unresolved_mark,
        module_exports: 0,
        exports: 0,
    };
    module.visit_with(&mut surface);
    surface.module_exports == 1 && surface.exports == 0
}

/// Whether `edge`'s writes land on the value `require` returned: the
/// consumer is CommonJS in the input, reads the value without an interop
/// helper or an `__esModule` test, and names neither `default` nor
/// `__esModule` in a write.
fn writes_the_value_itself(
    edge: &RequireEdge,
    consumer_writable: bool,
    uses: &RequireUses,
) -> bool {
    consumer_writable
        && edge.interop == Interop::None
        && !edge.esmodule_tested
        && !uses.interop_key_write
}

/// The decisions barrier 0 makes for every module of an unpack.
#[derive(Debug, Default)]
pub(crate) struct InterfacePlan {
    /// Providers a consumer writes to, by canonical key, with the first
    /// consumer that writes to each.
    written: HashMap<String, String>,
    /// The written providers that stay CommonJS: all but a whole-value
    /// provider whose every write lands on that value.
    kept: HashSet<String>,
    /// Modules whose required value was not a writable `exports` object.
    closed: HashSet<String>,
    /// The interop helpers of each provider no consumer writes to, by
    /// canonical key.
    helpers: HashMap<String, ProviderHelpers>,
    /// For each consumer, by canonical key: the sources it requires that
    /// resolve to a provider in `helpers`, with that provider's key.
    helper_sources: HashMap<String, Vec<(Atom, String)>>,
}

impl InterfacePlan {
    pub(crate) fn build<'a>(modules: impl IntoIterator<Item = PlanModule<'a>>) -> Self {
        let modules: Vec<PlanModule<'a>> = modules.into_iter().collect();
        let keys: HashSet<String> = modules
            .iter()
            .map(|module| canonical_module_key(module.filename))
            .collect();
        let closed: HashSet<String> = modules
            .iter()
            .filter(|module| !module.exports_object_writable)
            .map(|module| canonical_module_key(module.filename))
            .collect();
        let whole_value: HashSet<String> = modules
            .iter()
            .filter(|module| module.exports_whole_value)
            .map(|module| canonical_module_key(module.filename))
            .collect();
        let mut written: HashMap<String, String> = HashMap::default();
        let mut kept: HashSet<String> = HashSet::default();
        for module in &modules {
            for edge in module.edges.iter() {
                let Some(uses) = edge.uses.as_ref().filter(|uses| uses.writes()) else {
                    continue;
                };
                let Some(provider) = resolve_module_key(
                    |key| keys.contains(key),
                    Some(module.filename),
                    &edge.source,
                ) else {
                    continue;
                };
                if closed.contains(&provider) {
                    continue;
                }
                if !whole_value.contains(&provider)
                    || !writes_the_value_itself(edge, module.exports_object_writable, uses)
                {
                    kept.insert(provider.clone());
                }
                written
                    .entry(provider)
                    .or_insert_with(|| module.filename.to_string());
            }
        }
        let helpers: HashMap<String, ProviderHelpers> = modules
            .iter()
            .filter_map(|module| {
                let key = canonical_module_key(module.filename);
                (!written.contains_key(&key))
                    .then(|| module.helpers.map(|helpers| (key, helpers.clone())))
                    .flatten()
            })
            .collect();
        let mut helper_sources: HashMap<String, Vec<(Atom, String)>> = HashMap::default();
        if !helpers.is_empty() {
            for module in &modules {
                let mut sources: Vec<(Atom, String)> = Vec::new();
                for edge in module.edges.iter().chain(module.edges.unkeyed()) {
                    if sources.iter().any(|(source, _)| source == &edge.source) {
                        continue;
                    }
                    let Some(provider) = resolve_module_key(
                        |key| keys.contains(key),
                        Some(module.filename),
                        &edge.source,
                    ) else {
                        continue;
                    };
                    if helpers.contains_key(&provider) {
                        sources.push((edge.source.clone(), provider));
                    }
                }
                if !sources.is_empty() {
                    helper_sources.insert(canonical_module_key(module.filename), sources);
                }
            }
        }
        Self {
            written,
            kept,
            closed,
            helpers,
            helper_sources,
        }
    }

    /// Whether `UnEsm` keeps the module CommonJS because a consumer in the
    /// input writes to its `exports` object.
    pub(crate) fn keeps_commonjs(&self, filename: &str) -> bool {
        self.kept.contains(&canonical_module_key(filename))
    }

    /// Whether a consumer in the input writes to the module's required
    /// value, whether or not that keeps it CommonJS.
    pub(crate) fn is_written(&self, filename: &str) -> bool {
        self.written.contains_key(&canonical_module_key(filename))
    }

    /// The first consumer that writes to the module, when one does.
    pub(crate) fn written_by(&self, filename: &str) -> Option<&str> {
        self.written
            .get(&canonical_module_key(filename))
            .map(String::as_str)
    }

    /// The interop helpers of the providers `filename` requires, by the
    /// source it names each with.
    pub(crate) fn helper_sources(&self, filename: &str) -> HashMap<String, ProviderHelpers> {
        self.helper_sources
            .get(&canonical_module_key(filename))
            .into_iter()
            .flatten()
            .filter_map(|(source, provider)| {
                Some((source.to_string(), self.helpers.get(provider)?.clone()))
            })
            .collect()
    }

    /// The interop helpers the module exports, when it is not written.
    pub(crate) fn helper_exports(&self, filename: &str) -> Option<&ProviderHelpers> {
        self.helpers.get(&canonical_module_key(filename))
    }

    /// The interop helper `filename` reaches as `export` of what it
    /// requires from `source`.
    pub(crate) fn helper_kind(
        &self,
        filename: &str,
        source: &str,
        export: &CalleeExport,
    ) -> Option<TranspilerHelperKind> {
        let (_, provider) = self
            .helper_sources
            .get(&canonical_module_key(filename))?
            .iter()
            .find(|(required, _)| required.as_ref() == source)?;
        self.helpers.get(provider)?.kind_of(export)
    }

    /// Whether requiring the module returned a writable CommonJS `exports`
    /// object.
    pub(crate) fn exports_object_writable(&self, filename: &str) -> bool {
        !self.closed.contains(&canonical_module_key(filename))
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{parse_file_as_module, EsSyntax, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;
    use crate::rules::transpiler_helper_utils::LocalHelperContext;

    /// Parses and resolves `source`, then reads it with `read`.
    fn with_module<T>(source: &str, read: impl FnOnce(&Module, Mark) -> T) -> T {
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
            read(&module, mark)
        })
    }

    fn edges(source: &str) -> RequireEdges {
        with_module(source, |module, mark| {
            let mut edges = RequireEdges::collect(module, mark);
            edges.add_uses(
                module,
                mark,
                &LocalHelperContext::collect_with_mark(module, mark),
            );
            edges
        })
    }

    fn whole_value(source: &str) -> bool {
        with_module(source, exports_only_whole_value)
    }

    /// The plan for one consumer `src/consumer.js` beside the providers
    /// `src/<name>.js`; `closed` names the providers whose required value
    /// was not a writable `exports` object.
    fn plan(consumer: &str, providers: &[&str], closed: &[&str]) -> InterfacePlan {
        plan_with(consumer, true, providers, closed, &[])
    }

    /// [`plan`] with the consumer's own `exports` writable or not, and
    /// `whole` naming the providers whose only export is one
    /// `module.exports = v`.
    fn plan_with(
        consumer: &str,
        consumer_writable: bool,
        providers: &[&str],
        closed: &[&str],
        whole: &[&str],
    ) -> InterfacePlan {
        let consumer_edges = edges(consumer);
        let empty = RequireEdges::default();
        let names: Vec<String> = providers
            .iter()
            .map(|name| format!("src/{name}.js"))
            .collect();
        let mut modules = vec![PlanModule {
            filename: "src/consumer.js",
            edges: &consumer_edges,
            exports_object_writable: consumer_writable,
            helpers: None,
            exports_whole_value: false,
        }];
        for (name, filename) in providers.iter().zip(&names) {
            modules.push(PlanModule {
                filename,
                edges: &empty,
                exports_object_writable: !closed.contains(name),
                helpers: None,
                exports_whole_value: whole.contains(name),
            });
        }
        InterfacePlan::build(modules)
    }

    #[test]
    fn each_write_form_keeps_its_provider_commonjs() {
        let plan = plan(
            r#"var a = require("./assigned.js");
a.f = 1;
require("./inlined").g = 2;
var u = require("./updated.js");
if (x) { u.count++; }
delete require("./deleted.js").gone;
Object.assign(require("./assign-target.js"), extra);
Object.defineProperty(require("./defined.js"), "k", { get: k });
function install(m) { m.f = 1; }
install(require("./installed.js"));
var r = require("./read.js");
use(r.f);"#,
            &[
                "assigned",
                "inlined",
                "updated",
                "deleted",
                "assign-target",
                "defined",
                "installed",
                "read",
            ],
            &[],
        );
        for name in [
            "assigned",
            "inlined",
            "updated",
            "deleted",
            "assign-target",
            "defined",
            "installed",
        ] {
            let filename = format!("src/{name}.js");
            assert!(plan.keeps_commonjs(&filename), "{name}");
            assert_eq!(plan.written_by(&filename), Some("src/consumer.js"));
        }
        assert!(!plan.keeps_commonjs("src/read.js"));
    }

    #[test]
    fn escapes_that_prove_no_write_leave_the_provider_free() {
        let plan = plan(
            r#"function describe(m) { return Object.keys(m); }
describe(require("./local-reader.js"));
Object.keys(require("./builtin-reader.js"));
var installer = require("./installer.js");
installer.install(require("./cross-module.js"));
register(require("./unknown.js"));
__exportStar(require("./reexported.js"), exports);"#,
            &[
                "local-reader",
                "builtin-reader",
                "installer",
                "cross-module",
                "unknown",
                "reexported",
            ],
            &[],
        );
        for name in [
            "local-reader",
            "builtin-reader",
            "installer",
            "cross-module",
            "unknown",
            "reexported",
        ] {
            assert!(!plan.keeps_commonjs(&format!("src/{name}.js")), "{name}");
        }
    }

    #[test]
    fn a_provider_whose_exports_were_never_writable_is_left_alone() {
        let plan = plan(
            r#"require("./harmony.js").f = 1;
require("./plain.js").f = 1;"#,
            &["harmony", "plain"],
            &["harmony"],
        );
        assert!(!plan.keeps_commonjs("src/harmony.js"));
        assert!(!plan.exports_object_writable("src/harmony.js"));
        assert!(plan.keeps_commonjs("src/plain.js"));
        assert!(plan.exports_object_writable("src/plain.js"));
    }

    fn provider_helpers(source: &str) -> ProviderHelpers {
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
            ProviderHelpers::collect(&module, mark).expect("the provider exports a helper")
        })
    }

    #[test]
    fn a_written_provider_carries_no_helpers() {
        let helpers = provider_helpers(
            "module.exports = function (e) { return e && e.__esModule ? e : { default: e }; };",
        );
        let consumer = edges(
            r#"var h = require("./helper.js");
var w = require("./written.js");
w.f = 1;
h(require("./other.js"));"#,
        );
        let empty = RequireEdges::default();
        let plan = InterfacePlan::build([
            PlanModule {
                filename: "src/consumer.js",
                edges: &consumer,
                exports_object_writable: true,
                helpers: None,
                exports_whole_value: false,
            },
            PlanModule {
                filename: "src/helper.js",
                edges: &empty,
                exports_object_writable: true,
                helpers: Some(&helpers),
                exports_whole_value: false,
            },
            PlanModule {
                filename: "src/written.js",
                edges: &empty,
                exports_object_writable: true,
                helpers: Some(&helpers),
                exports_whole_value: false,
            },
        ]);
        assert!(plan.helper_exports("src/helper.js").is_some());
        assert!(plan.helper_exports("src/written.js").is_none());
        let sources = plan.helper_sources("src/consumer.js");
        assert_eq!(
            sources.keys().map(String::as_str).collect::<Vec<_>>(),
            ["./helper.js"]
        );
        assert!(plan.helper_sources("src/helper.js").is_empty());
        assert_eq!(
            plan.helper_kind("src/consumer.js", "./helper.js", &CalleeExport::Value),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
        assert_eq!(
            plan.helper_kind(
                "src/consumer.js",
                "./helper.js",
                &CalleeExport::Named("default".into())
            ),
            None
        );
        assert_eq!(
            plan.helper_kind("src/consumer.js", "./written.js", &CalleeExport::Value),
            None
        );
    }

    #[test]
    fn only_a_single_top_level_module_exports_value_is_a_whole_value_export() {
        // An empty registry object; core-js 2's `_global.js` (wild-observed,
        // webpack-bundled); core-js 3's `internals/shared-store.js` as a
        // minifier joins it (wild-observed, core-js@3.46.0); and an object
        // filled through its alias.
        for source in [
            "module.exports = {};",
            r#"var global = module.exports = typeof window != "undefined" && window.Math == Math ? window : typeof self != "undefined" && self.Math == Math ? self : Function("return this")();
if (typeof __g == "number") __g = global;"#,
            r#""use strict";
var n = require("./module-111.js"), o = require("./module-222.js"), i = require("./module-333.js"), c = "__core-js_shared__", a = module.exports = o[c] || i(c, {});
(a.versions || (a.versions = [])).push({ version: "3.46.0", mode: n ? "pure" : "global" });"#,
            "var o = module.exports = {}; o.f = function () {};",
        ] {
            assert!(whole_value(source), "{source}");
        }
        for source in [
            "exports.f = 1;",
            "module.exports = v; module.exports.x = 1;",
            "module.exports = v; Object.defineProperty(module.exports, \"__esModule\", { value: true });",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); module.exports = v;",
            "module.exports = v; install(exports);",
            "if (c) module.exports = v;",
            "module.exports = v; module.exports = w;",
            "module.exports = v; module.exports.default = module.exports;",
        ] {
            assert!(!whole_value(source), "{source}");
        }
    }

    #[test]
    fn a_whole_value_provider_written_on_its_value_is_not_kept() {
        let plan = plan_with(
            r#"var r = require("./registry.js");
r[key] = true;
r.flag = 1;
r.count++;
delete r.gone;
Object.assign(r, extra);
function install(m) { m.extra = 1; }
install(require("./registry.js"));
require("./registry.js").inline = 2;
var n = require("./named.js");
n.f = 1;"#,
            true,
            &["registry", "named"],
            &[],
            &["registry"],
        );
        assert!(!plan.keeps_commonjs("src/registry.js"));
        assert!(plan.is_written("src/registry.js"));
        assert_eq!(plan.written_by("src/registry.js"), Some("src/consumer.js"));
        // A provider that also has named exports keeps its `exports`
        // object, the only one a write can land on.
        assert!(plan.keeps_commonjs("src/named.js"));
    }

    #[test]
    fn a_write_that_may_miss_the_whole_value_keeps_its_provider() {
        let plan = plan_with(
            r#"var d = require("./default-key.js");
d.default = replacement;
Object.defineProperty(require("./marker.js"), "__esModule", { value: true });
function mark(m) { m.__esModule = true; }
mark(require("./marked-by-callee.js"));
function w(e) {
    if (e && e.__esModule) return e;
    var n = {};
    if (e != null) for (var k in e) if (Object.prototype.hasOwnProperty.call(e, k)) n[k] = e[k];
    n.default = e;
    return n;
}
var copy = w(require("./wildcard.js"));
copy.flag = 1;
var t = require("./tested.js");
if (!t.__esModule) t.flag = 1;"#,
            true,
            &[
                "default-key",
                "marker",
                "marked-by-callee",
                "wildcard",
                "tested",
            ],
            &[],
            &[
                "default-key",
                "marker",
                "marked-by-callee",
                "wildcard",
                "tested",
            ],
        );
        for name in [
            "default-key",
            "marker",
            "marked-by-callee",
            "wildcard",
            "tested",
        ] {
            assert!(plan.keeps_commonjs(&format!("src/{name}.js")), "{name}");
        }
    }

    #[test]
    fn a_write_from_an_es_module_consumer_keeps_a_whole_value_provider() {
        // A harmony consumer's require stands for a namespace import.
        let plan = plan_with(
            r#"var r = require("./registry.js");
r.flag = 1;"#,
            false,
            &["registry"],
            &[],
            &["registry"],
        );
        assert!(plan.keeps_commonjs("src/registry.js"));
    }

    #[test]
    fn a_written_whole_value_provider_carries_no_helpers() {
        let helpers = provider_helpers(
            "module.exports = function (e) { return e && e.__esModule ? e : { default: e }; };",
        );
        let consumer = edges(
            r#"var h = require("./helper.js");
h.patched = true;"#,
        );
        let empty = RequireEdges::default();
        let plan = InterfacePlan::build([
            PlanModule {
                filename: "src/consumer.js",
                edges: &consumer,
                exports_object_writable: true,
                helpers: None,
                exports_whole_value: false,
            },
            PlanModule {
                filename: "src/helper.js",
                edges: &empty,
                exports_object_writable: true,
                helpers: Some(&helpers),
                exports_whole_value: true,
            },
        ]);
        assert!(!plan.keeps_commonjs("src/helper.js"));
        assert!(plan.helper_exports("src/helper.js").is_none());
        assert!(plan.helper_sources("src/consumer.js").is_empty());
    }

    #[test]
    fn a_source_outside_the_input_is_not_planned() {
        let plan = plan(r#"require("lodash").f = 1;"#, &[], &[]);
        assert!(plan.written.is_empty());
    }
}
