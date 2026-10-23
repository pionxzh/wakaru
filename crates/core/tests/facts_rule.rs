mod common;

use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::facts::{
    collect_commonjs_default_attached_properties, collect_commonjs_default_object,
    collect_commonjs_value_is_default, collect_default_object_tail, collect_input_is_es_module,
    collect_marks_es_module, collect_module_facts, ExportFact, ExportKind, HelperExportFact,
    HelperKind, ImportFact, ImportKind, ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact,
    TypeScriptHelperKind,
};
use wakaru_core::{apply_rules, RulePipelineOptions};

/// Parse source, run Stage 1+2 (up through UnEsm), then collect facts.
fn collect_facts(source: &str) -> ModuleFacts {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("test.js".to_string()).into(),
            source.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax {
                jsx: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("parse failed");

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let commonjs_default_object = collect_commonjs_default_object(&module, unresolved_mark);
        let commonjs_default_attached_properties =
            collect_commonjs_default_attached_properties(&module, unresolved_mark);

        // Run pipeline through end of Stage 2
        apply_rules(
            &mut module,
            unresolved_mark,
            RulePipelineOptions::until("UnEsm"),
        );

        let mut facts = collect_module_facts(&module);
        facts.commonjs_default_object = commonjs_default_object;
        facts.commonjs_default_attached_properties = commonjs_default_attached_properties;
        facts
    })
}

/// Helper to build an ImportFact for assertions.
fn import(local: &str, source: &str, kind: ImportKind) -> ImportFact {
    ImportFact {
        local: local.into(),
        source: source.into(),
        kind,
    }
}

/// Helper to build an ExportFact for assertions.
fn export(exported: &str, local: Option<&str>, kind: ExportKind) -> ExportFact {
    ExportFact {
        exported: exported.into(),
        local: local.map(|s| s.into()),
        kind,
    }
}

fn helper_export(exported: &str, local: Option<&str>, kind: HelperKind) -> HelperExportFact {
    HelperExportFact {
        exported: exported.into(),
        local: local.map(|s| s.into()),
        kind,
    }
}

fn ts_helper_export(
    exported: &str,
    local: Option<&str>,
    kind: TypeScriptHelperKind,
) -> TypeScriptHelperExportFact {
    TypeScriptHelperExportFact {
        exported: exported.into(),
        local: local.map(|s| s.into()),
        kind,
    }
}

#[test]
fn module_facts_map_resolves_relative_specifiers_from_importer() {
    let mut map = ModuleFactsMap::new();
    map.insert("src/value.js", ModuleFacts::default());
    map.insert("shared/helper.js", ModuleFacts::default());

    assert!(map.get_from(Some("src/index.js"), "./value.js").is_some());
    assert!(map
        .get_from(Some("src/views/page.js"), "../../shared/helper.js")
        .is_some());
    assert!(map
        .get_from(Some("src/index.js"), "../../value.js")
        .is_none());
    assert!(map.get_from(Some("index.js"), "../value.js").is_none());
    assert!(map.get("./value.js").is_none());
}

// ── Import kind detection ──────────────────────────────────────────

#[test]
fn default_import() {
    let facts = collect_facts(r#"import x from "./mod";"#);
    assert_eq!(
        facts.imports,
        vec![import("x", "./mod", ImportKind::Default),]
    );
}

#[test]
fn namespace_import() {
    let facts = collect_facts(r#"import * as ns from "./mod";"#);
    assert_eq!(
        facts.imports,
        vec![import("ns", "./mod", ImportKind::Namespace),]
    );
}

#[test]
fn named_import() {
    let facts = collect_facts(r#"import { foo } from "./mod";"#);
    assert_eq!(
        facts.imports,
        vec![import("foo", "./mod", ImportKind::Named("foo".into())),]
    );
}

#[test]
fn named_import_with_alias() {
    let facts = collect_facts(r#"import { foo as bar } from "./mod";"#);
    assert_eq!(
        facts.imports,
        vec![import("bar", "./mod", ImportKind::Named("foo".into())),]
    );
}

#[test]
fn mixed_imports() {
    let facts = collect_facts(
        r#"
import def from "./a";
import * as ns from "./b";
import { x, y as z } from "./c";
"#,
    );
    assert_eq!(
        facts.imports,
        vec![
            import("def", "./a", ImportKind::Default),
            import("ns", "./b", ImportKind::Namespace),
            import("x", "./c", ImportKind::Named("x".into())),
            import("z", "./c", ImportKind::Named("y".into())),
        ]
    );
}

// ── CJS → ESM conversion ──────────────────────────────────────────

#[test]
fn require_becomes_default_import() {
    let facts = collect_facts(r#"var x = require("./mod");"#);
    assert_eq!(
        facts.imports,
        vec![import("x", "./mod", ImportKind::Default),]
    );
}

#[test]
fn interop_require_default_becomes_default_import() {
    let facts = collect_facts(
        r#"
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
var _mod = _interopRequireDefault(require("./mod"));
console.log(_mod.default);
"#,
    );
    // After Stage 2: helper unwrapped + UnEsm converts to import
    assert_eq!(facts.imports.len(), 1);
    assert_eq!(facts.imports[0].kind, ImportKind::Default);
    assert_eq!(facts.imports[0].source.as_ref(), "./mod");
}

// ── Export kind detection ──────────────────────────────────────────

#[test]
fn export_default_expr() {
    let facts = collect_facts(r#"export default 42;"#);
    assert_eq!(
        facts.exports,
        vec![export("default", None, ExportKind::Default),]
    );
}

#[test]
fn commonjs_passthrough_needs_the_whole_require_as_its_only_statement() {
    for source in [
        r#"module.exports = require("./provider.js")();"#,
        r#"module.exports = require("./provider.js").value;"#,
        r#"module.exports = require("./provider.js"); exports.x = 1;"#,
        r#"module.exports += require("./provider.js");"#,
    ] {
        assert_eq!(collect_facts(source).passthrough_target, None, "{source}");
    }
}

#[test]
fn use_strict_does_not_hide_a_commonjs_passthrough() {
    let facts = collect_facts(
        r#"
"use strict";
module.exports = require("./provider.js");
"#,
    );

    assert_eq!(
        facts.passthrough_target.as_deref(),
        Some("./provider.js"),
        "a preserved strict directive must not block transparent-provider facts"
    );
}

#[test]
fn export_default_function() {
    let facts = collect_facts(r#"export default function foo() {}"#);
    assert_eq!(
        facts.exports,
        vec![export("default", Some("foo"), ExportKind::Default),]
    );
}

#[test]
fn export_named_function() {
    let facts = collect_facts(r#"export function foo() {}"#);
    assert_eq!(
        facts.exports,
        vec![export("foo", Some("foo"), ExportKind::Named),]
    );
}

#[test]
fn export_named_const() {
    let facts = collect_facts(r#"export const a = 1, b = 2;"#);
    assert_eq!(
        facts.exports,
        vec![
            export("a", Some("a"), ExportKind::Named),
            export("b", Some("b"), ExportKind::Named),
        ]
    );
}

#[test]
fn export_named_class() {
    let facts = collect_facts(r#"export class Foo {}"#);
    assert_eq!(
        facts.exports,
        vec![export("Foo", Some("Foo"), ExportKind::Named),]
    );
}

#[test]
fn export_specifier_list() {
    let facts = collect_facts(
        r#"
const a = 1;
const b = 2;
export { a, b as c };
"#,
    );
    assert_eq!(
        facts.exports,
        vec![
            export("a", Some("a"), ExportKind::Named),
            export("c", Some("b"), ExportKind::Named),
        ]
    );
}

#[test]
fn export_default_via_specifier() {
    let facts = collect_facts(
        r#"
const a = 1;
export { a as default };
"#,
    );
    assert_eq!(
        facts.exports,
        vec![export("default", Some("a"), ExportKind::Default),]
    );
}

#[test]
fn source_reexport_has_no_local_binding() {
    let facts = collect_facts(r#"export { value as renamed } from "./dep.js";"#);
    assert_eq!(
        facts.exports,
        vec![export("renamed", None, ExportKind::Named),]
    );
}

// ── Helper export detection ───────────────────────────────────────

#[test]
fn async_to_generator_default_helper_export() {
    let facts = collect_facts(
        r#"
function step(gen, resolve, reject, next, throwFn, key, arg) {
    try {
        var info = gen[key](arg);
        var value = info.value;
    } catch (err) {
        reject(err);
        return;
    }
    if (info.done) {
        resolve(value);
    } else {
        Promise.resolve(value).then(next, throwFn);
    }
}
function asyncToGenerator(fn) {
    return function() {
        const self = this;
        const args = arguments;
        return new Promise((resolve, reject)=>{
            const gen = fn.apply(self, args);
            function next(value) {
                step(gen, resolve, reject, next, throwFn, "next", value);
            }
            function throwFn(value) {
                step(gen, resolve, reject, next, throwFn, "throw", value);
            }
            next(undefined);
        });
    };
}
export default asyncToGenerator;
"#,
    );
    assert_eq!(
        facts.helper_exports,
        vec![helper_export(
            "default",
            Some("asyncToGenerator"),
            HelperKind::AsyncToGenerator
        )]
    );
}

#[test]
fn babel_runtime_default_helper_export() {
    let facts = collect_facts(
        r#"
import _extends from "@babel/runtime/helpers/extends";
export default _extends;
"#,
    );
    assert_eq!(
        facts.helper_exports,
        vec![helper_export(
            "default",
            Some("_extends"),
            HelperKind::Extends
        )]
    );
}

#[test]
fn babel_runtime_named_default_helper_export() {
    let facts = collect_facts(
        r#"
import { default as _objectSpread2 } from "@babel/runtime/helpers/objectSpread2";
export { _objectSpread2 as objectSpread };
"#,
    );
    assert_eq!(
        facts.helper_exports,
        vec![helper_export(
            "objectSpread",
            Some("_objectSpread2"),
            HelperKind::ObjectSpread
        )]
    );
}

#[test]
fn exported_function_helper_fact() {
    let facts = collect_facts(
        r#"
export function Z() {
    return (Z = Object.assign ? Object.assign.bind() : function(target) {
        for (var i = 1; i < arguments.length; i++) {
            var source = arguments[i];
            for (var key in source) {
                if (Object.prototype.hasOwnProperty.call(source, key)) {
                    target[key] = source[key];
                }
            }
        }
        return target;
    }).apply(this, arguments);
}
"#,
    );
    assert_eq!(
        facts.helper_exports,
        vec![helper_export("Z", Some("Z"), HelperKind::Extends)]
    );
}

#[test]
fn default_object_helper_exports_use_property_names_for_aliases() {
    let facts = collect_facts(
        r#"
function n(strings, raw) {
    if (!raw) {
        raw = strings.slice(0);
    }
    return Object.freeze(Object.defineProperties(strings, {
        raw: {
            value: Object.freeze(raw)
        }
    }));
}
module.exports = {
    _: n,
    _tagged_template_literal: n
};
"#,
    );
    assert_eq!(
        facts.default_object_helper_exports,
        vec![
            helper_export("_", Some("n"), HelperKind::TaggedTemplateLiteral),
            helper_export(
                "_tagged_template_literal",
                Some("n"),
                HelperKind::TaggedTemplateLiteral
            ),
        ]
    );
}

#[test]
fn default_object_helper_exports_require_proven_local_helper_shape() {
    let facts = collect_facts(
        r#"
function fake(target, source) {
    console.log("side effect");
    return null;
}
module.exports = {
    _extends: fake
};
"#,
    );
    assert!(
        facts.default_object_helper_exports.is_empty(),
        "property name alone should not prove helper semantics"
    );
}

#[test]
fn empty_default_object_is_distinct_from_an_unknown_default_value() {
    let empty = collect_facts("module.exports = {};");
    let object = empty
        .commonjs_default_object
        .expect("an empty object assignment should still be proven");
    assert!(object.declared_properties.is_empty());

    let unknown = collect_facts("module.exports = makeProvider();");
    assert!(
        unknown.commonjs_default_object.is_none(),
        "an arbitrary factory result must remain unknown: {unknown}"
    );
}

#[test]
fn callable_default_properties_require_stable_unconditional_writes() {
    let facts = collect_facts(
        r#"
function api(value) { return value; }
api.parse = parse;
api.Rule = Rule;
module.exports = api;
api.default = api;
"#,
    );
    assert_eq!(
        facts.commonjs_default_attached_properties,
        vec!["Rule", "parse"],
        "only properties attached unconditionally before the CommonJS export should be proven"
    );

    let reassigned = collect_facts(
        r#"
function api(value) { return value; }
api.parse = parse;
api = decorate(api);
module.exports = api;
"#,
    );
    assert!(
        reassigned.commonjs_default_attached_properties.is_empty(),
        "a reassigned callable binding must fail closed: {reassigned}"
    );

    let conditional = collect_facts(
        r#"
function api(value) { return value; }
if (enabled) api.parse = parse;
module.exports = api;
"#,
    );
    assert!(
        conditional.commonjs_default_attached_properties.is_empty(),
        "a conditional property write does not prove the callable surface: {conditional}"
    );
}

#[test]
fn callable_default_properties_reject_a_conditional_default_rewrite() {
    let facts = collect_facts(
        r#"
function api(value) { return value; }
api.parse = function(value) { return value.length; };
module.exports = api;
if (globalThis.legacy) { module.exports = globalThis.wrap(api); }
"#,
    );
    assert!(
        facts.commonjs_default_attached_properties.is_empty(),
        "a second module.exports assignment must make the callable default unknown: {facts}"
    );
}

#[test]
fn default_object_properties_follow_a_stable_local_alias() {
    let facts = collect_facts(
        r#"
var methods = {
    map: function(value) { return value; },
    filter(value) { return value; },
    get size() { return 2; },
    [dynamicKey]: dynamicValue,
    ...extra
};
module.exports = methods;
methods.enabled = true;
"#,
    );
    assert_eq!(
        facts
            .commonjs_default_object
            .expect("the stable alias should prove the default object")
            .declared_properties,
        vec!["filter", "map", "size"]
    );
}

#[test]
fn default_object_proto_entry_is_not_a_declared_property() {
    let facts = collect_facts("module.exports = { __proto__: base, real: 1 };");
    assert_eq!(
        facts
            .commonjs_default_object
            .expect("the literal assignment should still prove the default object")
            .declared_properties,
        vec!["real"],
        "a plain __proto__ entry sets the prototype, not an own property"
    );
}

#[test]
fn callable_default_prototype_write_is_not_an_attached_property() {
    let facts = collect_facts(
        r#"
function api(value) { return value; }
api.__proto__ = base;
api.parse = parse;
module.exports = api;
"#,
    );
    assert_eq!(
        facts.commonjs_default_attached_properties,
        vec!["parse"],
        "a prototype write on the callable is not part of the export surface"
    );
}

#[test]
fn default_object_properties_reject_a_reassigned_alias() {
    let facts = collect_facts(
        r#"
var methods = { map: function(value) { return value; } };
methods = decorate(methods);
module.exports = methods;
"#,
    );
    assert!(
        facts.commonjs_default_object.is_none(),
        "a reassigned alias does not prove the final default value's properties: {facts}"
    );
}

#[test]
fn default_object_properties_distinguish_exports_default() {
    let facts = collect_facts(
        r#"
exports.default = {
    map: function(value) { return value; }
};
"#,
    );
    assert!(
        facts.commonjs_default_object.is_none(),
        "exports.default is a namespace property, not the CommonJS value itself: {facts}"
    );
}

#[test]
fn default_object_properties_reject_multiple_module_exports_assignments() {
    let facts = collect_facts(
        r#"
module.exports = { map: first };
module.exports = { map: second };
"#,
    );
    assert!(
        facts.commonjs_default_object.is_none(),
        "multiple whole-value assignments require control-flow reasoning: {facts}"
    );
}

#[test]
fn export_all_marks_the_provider_surface_as_open() {
    let facts = collect_facts(
        r#"
module.exports = { local: 1 };
export * from "./other.js";
"#,
    );
    assert!(facts.has_export_all);
    assert_eq!(
        facts
            .commonjs_default_object
            .expect("the direct assignment should prove the default object")
            .declared_properties,
        vec!["local"]
    );
}

#[test]
fn tagged_template_literal_helper_export_detects_swc_shape() {
    let facts = collect_facts(
        r#"
function Y(strings, raw) {
    if (!raw) {
        raw = strings.slice(0);
    }
    return Object.freeze(Object.defineProperties(strings, {
        raw: {
            value: Object.freeze(raw)
        }
    }));
}
export { Y as _ };
"#,
    );
    assert_eq!(
        facts.helper_exports,
        vec![helper_export(
            "_",
            Some("Y"),
            HelperKind::TaggedTemplateLiteral
        )]
    );
}

#[test]
fn regenerator_runtime_default_helper_export() {
    let facts = collect_facts(
        r#"
const runtime = require("./runtime.js")();
export default runtime;
try {
    regeneratorRuntime = runtime;
} catch (err) {
    globalThis.regeneratorRuntime = runtime;
}
"#,
    );
    assert_eq!(
        facts.helper_exports,
        vec![helper_export(
            "default",
            Some("runtime"),
            HelperKind::RegeneratorRuntime
        )]
    );
}

#[test]
fn exported_typescript_awaiter_helper_fact() {
    let facts = collect_facts(
        r#"
export var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    return new (P || (P = Promise))(function(resolve) {
        resolve(generator.apply(thisArg, _arguments || []).next());
    });
};
"#,
    );
    assert_eq!(
        facts.ts_helper_exports,
        vec![ts_helper_export(
            "__awaiter",
            Some("__awaiter"),
            TypeScriptHelperKind::Awaiter
        )]
    );
}

#[test]
fn exported_typescript_helper_fact_requires_inline_shape() {
    let facts = collect_facts(
        r#"
var __assign = (this && this.__assign) || customAssign;
export { __assign };
"#,
    );
    assert!(
        facts.ts_helper_exports.is_empty(),
        "name-only inline helper candidates must not become proven facts: {facts}"
    );
}

#[test]
fn exported_typescript_spread_array_alias_helper_fact() {
    let facts = collect_facts(
        r#"
var __spreadArray = (this && this.__spreadArray) || function (to, from, pack) {
    return to.concat(from);
};
export { __spreadArray as spreadArray };
"#,
    );
    assert_eq!(
        facts.ts_helper_exports,
        vec![ts_helper_export(
            "spreadArray",
            Some("__spreadArray"),
            TypeScriptHelperKind::SpreadArray
        )]
    );
}

#[test]
fn exported_typescript_public_function_helper_fact() {
    let facts = collect_facts(
        r#"
function helper(source, excluded) {
    var target = {};
    for (var key in source) {
        if (Object.prototype.hasOwnProperty.call(source, key) && excluded.indexOf(key) < 0) {
            target[key] = source[key];
        }
    }
    if (source != null && typeof Object.getOwnPropertySymbols === "function") {
        for (var i = 0, key = Object.getOwnPropertySymbols(source); i < key.length; i++) {
            if (excluded.indexOf(key[i]) < 0 && Object.prototype.propertyIsEnumerable.call(source, key[i])) {
                target[key[i]] = source[key[i]];
            }
        }
    }
    return target;
}
export { helper as __rest };
"#,
    );
    assert_eq!(
        facts.ts_helper_exports,
        vec![ts_helper_export(
            "__rest",
            Some("helper"),
            TypeScriptHelperKind::Rest
        )]
    );
}

#[test]
fn exported_typescript_public_function_helper_fact_requires_shape() {
    let facts = collect_facts(
        r#"
export function __rest(source, excluded) {
    return customRest(source, excluded);
}
"#,
    );
    assert!(
        facts.ts_helper_exports.is_empty(),
        "public helper names without matching helper bodies must not become proven facts: {facts}"
    );
}

#[test]
fn exported_tslib_import_star_function_fact() {
    // tslib 2.8.1 `tslib.es6.mjs`, and the same module after webpack@5.111.1
    // with Terser, which drops the dependency names.
    let readable = r#"
var __createBinding = function(o, m, k, k2) { o[k2 === undefined ? k : k2] = m[k]; };
var __setModuleDefault = function(o, v) { o["default"] = v; };
var ownKeys = function(o) { return Object.getOwnPropertyNames(o); };
export function __importStar(mod) {
  if (mod && mod.__esModule) return mod;
  var result = {};
  if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
  __setModuleDefault(result, mod);
  return result;
}
"#;
    let minified = r#"
var n = function(e, t, r, n) { e[void 0 === n ? r : n] = t[r]; },
    o = function(e, t) { e.default = t; },
    u = function(e) { return Object.getOwnPropertyNames(e); };
function i(e){if(e&&e.__esModule)return e;var t={};if(null!=e)for(var r=u(e),i=0;i<r.length;i++)"default"!==r[i]&&n(t,e,r[i]);return o(t,e),t}
export { i as __importStar };
"#;
    for (source, local) in [(readable, "__importStar"), (minified, "i")] {
        let facts = collect_facts(source);
        assert_eq!(
            facts.ts_helper_exports,
            vec![ts_helper_export(
                "__importStar",
                Some(local),
                TypeScriptHelperKind::ImportStar
            )]
        );
    }
}

#[test]
fn exported_tslib_import_star_function_fact_requires_name_and_shape() {
    for source in [
        // The public name over another body.
        r#"
export function __importStar(mod) {
  if (mod && mod.__esModule) return mod;
  return { default: mod };
}
"#,
        // The body under another name.
        r#"
function wrap(mod) {
  if (mod && mod.__esModule) return mod;
  var result = {};
  for (var k in mod) if (k !== "default") result[k] = mod[k];
  result["default"] = mod;
  return result;
}
export { wrap };
"#,
    ] {
        let facts = collect_facts(source);
        assert!(
            facts.ts_helper_exports.is_empty(),
            "only the public name with the matching body is a proven helper: {facts}"
        );
    }
}

#[test]
fn anonymous_default_export_helper_fact() {
    // A minifier drops the helper's name: `module.exports = function (e) {…}`,
    // as Terser leaves @babel/runtime's `interopRequireDefault`.
    for source in [
        "module.exports = function(e) { return e && e.__esModule ? e : { default: e }; };",
        "export default (e) => e && e.__esModule ? e : { default: e };",
    ] {
        let facts = collect_facts(source);
        assert_eq!(
            facts.helper_exports,
            vec![helper_export(
                "default",
                None,
                HelperKind::InteropRequireDefault
            )]
        );
    }
}

#[test]
fn anonymous_default_export_fact_requires_helper_shape() {
    let facts = collect_facts("module.exports = function(e) { return { default: e }; };");
    assert!(
        facts.helper_exports.is_empty(),
        "an anonymous default export without a helper body is not a helper: {facts}"
    );
}

#[test]
fn marks_es_module_reads_the_flag_inside_a_top_level_sequence() {
    let marks = |source: &str| {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(
                FileName::Custom("test.js".to_string()).into(),
                source.to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*fm),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("parse failed");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            collect_marks_es_module(&module, unresolved_mark)
        })
    };
    // shape: producer webpack@5.111.1 mode=production (Terser) over a tsc
    // CommonJS module.
    assert!(marks(
        r#""use strict";Object.defineProperty(exports,"__esModule",{value:!0}),exports.default="value";"#
    ));
    assert!(!marks(
        r#"if (ready) Object.defineProperty(exports,"__esModule",{value:!0}),exports.default="value";"#
    ));
    assert!(!marks(
        r#"(function(){Object.defineProperty(exports,"__esModule",{value:!0}),exports.default="value"})();"#
    ));
}

#[test]
fn commonjs_value_is_default_needs_one_top_level_module_exports_assignment() {
    let is_default = |source: &str| {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(
                FileName::Custom("test.js".to_string()).into(),
                source.to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*fm),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("parse failed");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            collect_commonjs_value_is_default(&module, unresolved_mark)
        })
    };
    assert!(is_default("module.exports = function (t) { return t; };"));
    // producer metro@0.87.0 minified (`metro-runtime` asyncRequire): the
    // assignment joined into a top-level sequence.
    assert!(is_default(
        "function u() {} u.prefetch = p, module.exports = u;"
    ));
    assert!(!is_default("c && (u.x = 1, module.exports = u);"));
    // producer webpack@4.47.0 mode=production over the `process` browser
    // shim.
    assert!(is_default(
        "var n, r, o = module.exports = {}; o.title = \"browser\";"
    ));
    assert!(is_default("exports = module.exports = UAParser;"));
    assert!(!is_default("function f() { var o = module.exports = {}; }"));
    assert!(is_default(
        r#""use strict"; function f() {} f.version = 1; module.exports = f; module.exports.x = 2;"#
    ));
    // Without the assignment the value is the `exports` object.
    assert!(!is_default("exports.a = 1;"));
    assert!(!is_default("module.exports = a; module.exports = b;"));
    assert!(!is_default("if (c) module.exports = a;"));
    assert!(!is_default(
        "module.exports = a; function reset() { module.exports = b; }"
    ));
    assert!(!is_default("module.exports = a; module.hot.accept();"));
    assert!(!is_default("module.exports = a; eval(code);"));
    assert!(!is_default("export default a;"));
}

#[test]
fn input_is_es_module_needs_a_declaration_and_no_commonjs_global() {
    let is_esm = |source: &str, commonjs_value_is_default: bool| {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(
                FileName::Custom("test.js".to_string()).into(),
                source.to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*fm),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("parse failed");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            collect_input_is_es_module(&module, unresolved_mark, commonjs_value_is_default)
        })
    };
    assert!(is_esm("export const f = 1; export default g;", false));
    assert!(is_esm(r#"import x from "./x"; use(x);"#, false));
    // A local named `module` is no CommonJS global.
    assert!(is_esm(
        "export default function (module) { return module.id; }",
        false
    ));
    assert!(!is_esm("exports.f = 1;", false));
    // A CommonJS runtime normalizer's `export default`.
    assert!(!is_esm("export default fn;", true));
    assert!(!is_esm("export default fn; exports.beta = 2;", false));
    assert!(!is_esm("export const f = 1; module.hot.accept();", false));
}

#[test]
fn default_object_tail_makes_the_commonjs_value_the_default() {
    let has_tail = |source: &str| {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(
                FileName::Custom("test.js".to_string()).into(),
                source.to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*fm),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("parse failed");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            collect_default_object_tail(&module, unresolved_mark)
        })
    };
    let getter = |value: &str| {
        format!(
            r#"Object.defineProperty(exports, "__esModule", {{ value: true }}); Object.defineProperty(exports, "default", {{ enumerable: true, get: function () {{ return D; }} }}); {value}"#
        )
    };
    let tail = r#"if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && typeof exports.default.__esModule === "undefined") { Object.defineProperty(exports.default, "__esModule", { value: true }); Object.assign(exports.default, exports); module.exports = exports.default; }"#;
    // producer next@16.3.8 dist (swc) unminified, and the same tail minified
    // with the getter joined into a sequence.
    assert!(has_tail(&format!("{} {tail}", getter("function D() {}"))));
    assert!(has_tail(&format!(
        "{} {tail}",
        getter("var D = wrap(function () {});")
    )));
    assert!(has_tail(
        r#"require("./x"), Object.defineProperty(exports, "__esModule", { value: !0 }), Object.defineProperty(exports, "default", { enumerable: !0, get: function () { return D; } }); var D = make(); ("function" == typeof exports.default || "object" == typeof exports.default && null !== exports.default) && void 0 === exports.default.__esModule && (Object.defineProperty(exports.default, "__esModule", { value: !0 }), Object.assign(exports.default, exports), module.exports = exports.default);"#
    ));
    // A primitive default keeps `module.exports` the exports object.
    assert!(!has_tail(&format!(
        "{} {tail}",
        getter(r#"var D = "label";"#)
    )));
    assert!(!has_tail(&format!(
        r#"exports.default = void 0; exports.default = 3; {tail}"#
    )));
    // The tail must end the module and hold its only reassignment.
    assert!(!has_tail(&format!(
        "{} {tail} sideEffect();",
        getter("function D() {}")
    )));
    assert!(!has_tail(&format!(
        "{} module.exports = other; {tail}",
        getter("function D() {}")
    )));
    assert!(!has_tail(&format!(
        "{} if (c) {{ module.exports = exports.default; }}",
        getter("function D() {}")
    )));
}

#[test]
fn registered_typescript_values_helper_fact() {
    let facts = collect_facts(
        r#"
function __values(o) {
  var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
  if (m) return m.call(o);
  if (o && typeof o.length === "number") return {
    next: function() {
      if (o && i >= o.length) o = void 0;
      return { value: o && o[i++], done: !o };
    }
  };
  throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
}
function register(name, value) {
  exports[name] = value;
}
register("__values", __values);
"#,
    );
    assert_eq!(
        facts.ts_helper_exports,
        vec![ts_helper_export(
            "__values",
            Some("__values"),
            TypeScriptHelperKind::Values
        )]
    );
}

#[test]
fn registered_nested_typescript_values_helper_fact() {
    let facts = collect_facts(
        r#"
export function tslibModule() {
  function register(name, value) {
    exports[name] = value;
  }
  function __values(o) {
    var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
    if (m) return m.call(o);
    if (o && typeof o.length === "number") return {
      next: function() {
        if (o && i >= o.length) o = void 0;
        return { value: o && o[i++], done: !o };
      }
    };
    throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
  }
  register("__values", __values);
  return exports;
}
"#,
    );
    assert_eq!(
        facts.ts_helper_exports,
        vec![ts_helper_export(
            "__values",
            Some("__values"),
            TypeScriptHelperKind::Values
        )]
    );
}

#[test]
fn registered_assigned_typescript_values_helper_fact() {
    let facts = collect_facts(
        r#"
export function tslibModule() {
  let q;
  q = (name, value) => {
    module.exports[name] = value;
  };
  let ZM8;
  ZM8 = (o) => {
    const s = typeof Symbol === "function" && Symbol.iterator;
    const m = s && o[s];
    let i = 0;
    if (m) {
      return m.call(o);
    }
    if (o && typeof o.length === "number") {
      return {
        next() {
          if (o && i >= o.length) {
            o = undefined;
          }
          return {
            value: o && o[i++],
            done: !o
          };
        }
      };
    }
    throw TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
  };
  q("__values", ZM8);
  return exports;
}
"#,
    );
    assert_eq!(
        facts.ts_helper_exports,
        vec![ts_helper_export(
            "__values",
            Some("ZM8"),
            TypeScriptHelperKind::Values
        )]
    );
    assert_eq!(
        facts.ts_helper_namespace_factory_exports,
        vec!["tslibModule"]
    );
}

#[test]
fn minified_async_helpers_prove_namespace_factory_fact() {
    let facts = collect_facts(
        r#"
export function helperFactory() {
  const exportsObject = {};
  const module = { exports: exportsObject };
  let m2q = (thisArg, args, PromiseImpl, generator) => {
    return new (PromiseImpl || (PromiseImpl = Promise))((resolve, reject) => {
      function step(result) {
        if (result.done) resolve(result.value);
        else Promise.resolve(result.value).then(fulfilled, rejected);
      }
      function fulfilled(value) { step(generator.next(value)); }
      function rejected(error) { step(generator.throw(error)); }
      step((generator = generator.apply(thisArg, args || [])).next());
    });
  };
  let p2q = (thisArg, body) => {
    const state = { label: 0, trys: [], ops: [] };
    return state;
  };
  function register(name, value) {
    module.exports[name] = value;
  }
  register("__awaiter", m2q);
  register("__generator", p2q);
  return module.exports;
}
"#,
    );

    assert!(facts.ts_helper_exports.iter().any(|helper| {
        helper.exported.as_ref() == "__awaiter" && helper.kind == TypeScriptHelperKind::Awaiter
    }));
    assert!(facts.ts_helper_exports.iter().any(|helper| {
        helper.exported.as_ref() == "__generator" && helper.kind == TypeScriptHelperKind::Generator
    }));
    assert_eq!(
        facts.ts_helper_namespace_factory_exports,
        vec!["helperFactory"]
    );
}

#[test]
fn registered_typescript_values_helper_fact_from_umd_registrar_factory() {
    let facts = collect_facts(
        r#"
(function(callback) {
  const globalTarget = {};
  const moduleExports = {};
  function makeRegistrar(target, adapter) {
    return (name, value) => target[name] = adapter ? adapter(name, value) : value;
  }
  callback(makeRegistrar(globalTarget, makeRegistrar(moduleExports)));
})((register) => {
  let ZM8;
  ZM8 = (o) => {
    const s = typeof Symbol === "function" && Symbol.iterator;
    const m = s && o[s];
    let i = 0;
    if (m) {
      return m.call(o);
    }
    if (o && typeof o.length === "number") {
      return {
        next() {
          if (o && i >= o.length) {
            o = undefined;
          }
          return {
            value: o && o[i++],
            done: !o
          };
        }
      };
    }
    throw TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
  };
  register("__values", ZM8);
});
"#,
    );
    assert_eq!(
        facts.ts_helper_exports,
        vec![ts_helper_export(
            "__values",
            Some("ZM8"),
            TypeScriptHelperKind::Values
        )]
    );
}

#[test]
fn registered_typescript_values_helper_fact_ignores_nested_uninvoked_callback() {
    let facts = collect_facts(
        r#"
(function(callback) {
  function later() {
    callback((name, value) => exports[name] = value);
  }
})((register) => {
  function __values(o) {
    var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
    if (m) return m.call(o);
    if (o && typeof o.length === "number") return {
      next: function() {
        if (o && i >= o.length) o = void 0;
        return { value: o && o[i++], done: !o };
      }
    };
    throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
  }
  register("__values", __values);
});
"#,
    );
    assert!(
        facts.ts_helper_exports.is_empty(),
        "registrars passed only inside nested functions must not become helper facts: {facts}"
    );
}

#[test]
fn registered_typescript_values_helper_fact_requires_factory_adapter_to_return_value() {
    let facts = collect_facts(
        r#"
(function(callback) {
  const globalTarget = {};
  function makeRegistrar(target, adapter) {
    return (name, value) => target[name] = adapter ? adapter(name, value) : value;
  }
  callback(makeRegistrar(globalTarget, (name, value) => null));
})((register) => {
  function __values(o) {
    var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
    if (m) return m.call(o);
    if (o && typeof o.length === "number") return {
      next: function() {
        if (o && i >= o.length) o = void 0;
        return { value: o && o[i++], done: !o };
      }
    };
    throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
  }
  register("__values", __values);
});
"#,
    );
    assert!(
        facts.ts_helper_exports.is_empty(),
        "factory adapters that do not return the registered value must not become helper facts: {facts}"
    );
}

#[test]
fn registered_typescript_values_helper_fact_requires_inline_factory_adapter_to_return_value() {
    let facts = collect_facts(
        r#"
(function(callback) {
  const globalTarget = {};
  callback(((target, adapter) =>
    (name, value) => target[name] = adapter ? adapter(name, value) : value
  )(globalTarget, (name, value) => null));
})((register) => {
  function __values(o) {
    var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
    if (m) return m.call(o);
    if (o && typeof o.length === "number") return {
      next: function() {
        if (o && i >= o.length) o = void 0;
        return { value: o && o[i++], done: !o };
      }
    };
    throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
  }
  register("__values", __values);
});
"#,
    );
    assert!(
        facts.ts_helper_exports.is_empty(),
        "inline factory adapters that do not return the registered value must not become helper facts: {facts}"
    );
}

#[test]
fn registered_typescript_values_helper_fact_requires_export_registrar() {
    let facts = collect_facts(
        r#"
function __values(o) {
  var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
  if (m) return m.call(o);
  if (o && typeof o.length === "number") return {
    next: function() {
      if (o && i >= o.length) o = void 0;
      return { value: o && o[i++], done: !o };
    }
  };
  throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
}
function customValues(items) {
  return { next: function() { return { done: true }; } };
}
function unrelatedFactory() {
  return { __values: customValues };
}
logMetric("__values", __values);
export { unrelatedFactory };
"#,
    );
    assert!(
        facts.ts_helper_exports.is_empty(),
        "calls that do not write to exports must not register helper facts: {facts}"
    );
}

// ── CJS exports → ESM ──────────────────────────────────────────────

#[test]
fn module_exports_becomes_default_export() {
    let facts = collect_facts(r#"module.exports = { foo: 1 };"#);
    assert!(!facts.exports.is_empty());
    assert!(
        facts.exports.iter().any(|e| e.kind == ExportKind::Default),
        "should have a default export, got: {facts}"
    );
}

#[test]
fn exports_dot_name_becomes_named_export() {
    let facts = collect_facts(
        r#"
exports.foo = function() {};
exports.bar = 42;
"#,
    );
    assert!(
        facts
            .exports
            .iter()
            .any(|e| e.exported.as_ref() == "foo" && e.kind == ExportKind::Named),
        "should have named export 'foo', got: {facts}"
    );
    assert!(
        facts
            .exports
            .iter()
            .any(|e| e.exported.as_ref() == "bar" && e.kind == ExportKind::Named),
        "should have named export 'bar', got: {facts}"
    );
}

// ── No imports or exports ──────────────────────────────────────────

#[test]
fn plain_code_has_empty_facts() {
    let facts = collect_facts(r#"console.log("hello");"#);
    assert!(facts.imports.is_empty());
    assert!(facts.exports.is_empty());
}

// ── Display ────────────────────────────────────────────────────────

#[test]
fn display_formatting() {
    let facts = collect_facts(
        r#"
import x from "./a";
import { foo as bar } from "./b";
export const val = 1;
export default 42;
"#,
    );
    let display = format!("{facts}");
    assert!(
        display.contains("import x from \"./a\" [default]"),
        "got: {display}"
    );
    assert!(
        display.contains("import bar from \"./b\" [named(foo)]"),
        "got: {display}"
    );
    assert!(display.contains("export val [named]"), "got: {display}");
    assert!(
        display.contains("export default [default]"),
        "got: {display}"
    );
}

// ── Side-effect-only import ────────────────────────────────────────

#[test]
fn side_effect_import_produces_no_bindings() {
    let facts = collect_facts(r#"import "./side-effect";"#);
    // No specifiers → no import facts
    assert!(facts.imports.is_empty());
}

#[test]
fn is_helper_module_true_for_mapped_helper_export() {
    let facts = collect_facts(
        r#"
function _extends() {
    _extends = Object.assign || function(target) {
        for (var i = 1; i < arguments.length; i++) {
            var source = arguments[i];
            for (var key in source) {
                if (Object.prototype.hasOwnProperty.call(source, key)) {
                    target[key] = source[key];
                }
            }
        }
        return target;
    };
    return _extends.apply(this, arguments);
}
export default _extends;
"#,
    );
    assert!(!facts.helper_exports.is_empty());
    assert!(facts.is_helper_module);
}

#[test]
fn is_helper_module_true_for_unmapped_helper_dependency() {
    // `_defineProperty` is a recognized transpiler helper but maps to no rewrite
    // HelperKind, so it never appears in `helper_exports`. It must still be
    // flagged as a helper module so dead-module elimination can treat it as
    // removable boilerplate.
    let facts = collect_facts(
        r#"
function _defineProperty(obj, key, value) {
    if (key in obj) {
        Object.defineProperty(obj, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        obj[key] = value;
    }
    return obj;
}
export default _defineProperty;
"#,
    );
    assert!(
        facts.helper_exports.is_empty(),
        "defineProperty has no rewrite-mapped helper export"
    );
    assert!(
        facts.is_helper_module,
        "defineProperty should still be recognized as a helper module"
    );
}

#[test]
fn is_helper_module_false_for_plain_module() {
    let facts = collect_facts(r#"export const value = compute();"#);
    assert!(!facts.is_helper_module);
}

// ============================================================
// Re-export consolidation: bare uses of the default import
// ============================================================

fn consolidate_reexports(source: &str, facts: &ModuleFactsMap, filename: &str) -> String {
    use swc_core::ecma::transforms::base::fixer::fixer;
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = common::parse_module_with_filename(source, filename, cm.clone());
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        wakaru_core::reexport_consolidation::run_reexport_consolidation(
            &mut module,
            facts,
            Some(filename),
        );
        module.visit_mut_with(&mut fixer(None));
        common::emit_module(&module, cm)
    })
}

fn passthrough_facts() -> ModuleFactsMap {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "passthrough.js",
        ModuleFacts {
            passthrough_target: Some("./provider.js".into()),
            ..Default::default()
        },
    );
    facts.insert("provider.js", ModuleFacts::default());
    facts
}

#[test]
fn member_only_default_import_is_redirected_to_the_provider_namespace() {
    let output = consolidate_reexports(
        r#"
import lib from "./passthrough.js";
use(lib.foo);
"#,
        &passthrough_facts(),
        "consumer.js",
    );
    assert!(
        output.contains(r#"import * as lib from "./provider.js""#),
        "{output}"
    );
}

#[test]
fn default_import_read_bare_in_a_computed_key_is_not_redirected() {
    // `[lib]` reads the binding itself, not a member: a namespace object
    // would change the value.
    let output = consolidate_reexports(
        r#"
import lib from "./passthrough.js";
use(lib.foo);
const keys = { [lib]: 1 };
"#,
        &passthrough_facts(),
        "consumer.js",
    );
    assert!(
        output.contains(r#"import lib from "./passthrough.js""#),
        "{output}"
    );
    assert!(!output.contains("import * as"), "{output}");
}

fn passthrough_facts_with_commonjs_provider() -> ModuleFactsMap {
    let mut facts = passthrough_facts();
    facts.insert(
        "provider.js",
        ModuleFacts {
            stays_commonjs: true,
            ..Default::default()
        },
    );
    facts
}

#[test]
fn default_import_of_a_passthrough_to_a_commonjs_provider_stays_a_default_import() {
    // The passthrough's value is the provider's `module.exports`, which is
    // the provider's default import. Its namespace holds only the names
    // Node's lexer finds, so `lib.version` on `module.exports = fn` would
    // read `undefined` there.
    for source in [
        "import lib from \"./passthrough.js\";\nuse(lib.version);\n",
        "import lib from \"./passthrough.js\";\nuse(lib);\n",
    ] {
        let output = consolidate_reexports(
            source,
            &passthrough_facts_with_commonjs_provider(),
            "consumer.js",
        );
        assert!(
            output.contains(r#"import lib from "./provider.js""#),
            "{output}"
        );
        assert!(!output.contains("import * as"), "{output}");
    }
}

#[test]
fn default_import_of_a_passthrough_to_an_unknown_target_is_not_redirected() {
    // Without facts for the target, its format is unknown: neither a default
    // nor a namespace import of it is proven to hold the same value.
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "passthrough.js",
        ModuleFacts {
            passthrough_target: Some("external-lib".into()),
            ..Default::default()
        },
    );
    let output = consolidate_reexports(
        "import lib from \"./passthrough.js\";\nuse(lib.foo);\n",
        &facts,
        "consumer.js",
    );
    assert!(
        output.contains(r#"import lib from "./passthrough.js""#),
        "{output}"
    );
}

#[test]
fn destructured_default_import_is_redirected_to_the_provider_namespace() {
    // Destructuring with fixed keys reads members, as `lib.a` does.
    for source in [
        "import lib from \"./passthrough.js\";\nconst { a, b: c, d = 1 } = lib;\nuse(a, c, d);\n",
        "import lib from \"./passthrough.js\";\nlet a;\n({ a } = lib);\nuse(a);\n",
    ] {
        let output = consolidate_reexports(source, &passthrough_facts(), "consumer.js");
        assert!(
            output.contains(r#"import * as lib from "./provider.js""#),
            "{output}"
        );
    }
}

#[test]
fn destructured_default_import_with_a_rest_or_computed_key_is_not_redirected() {
    // A rest element copies the object's own keys, and a computed key is
    // not a fixed member: the namespace object can differ in both.
    for source in [
        "import lib from \"./passthrough.js\";\nconst { a, ...rest } = lib;\nuse(a, rest);\n",
        "import lib from \"./passthrough.js\";\nconst { [key]: a } = lib;\nuse(a);\n",
        "import lib from \"./passthrough.js\";\nconst [a] = lib;\nuse(a);\n",
    ] {
        let output = consolidate_reexports(source, &passthrough_facts(), "consumer.js");
        assert!(
            output.contains(r#"import lib from "./passthrough.js""#),
            "{output}"
        );
    }
}

fn passthrough_facts_with_provider(provider: ModuleFacts) -> ModuleFactsMap {
    let mut facts = passthrough_facts();
    facts.insert("provider.js", provider);
    facts
}

#[test]
fn default_import_of_a_passthrough_to_a_default_only_provider_now_esm_stays_a_default_import() {
    // The provider was `module.exports = fn` and is now `export default fn`:
    // its namespace holds `fn` only as `default`.
    let facts = passthrough_facts_with_provider(ModuleFacts {
        commonjs_value_is_default: true,
        exports: vec![export("default", None, ExportKind::Default)],
        ..Default::default()
    });
    let output = consolidate_reexports(
        "import lib from \"./passthrough.js\";\nuse(lib.version);\n",
        &facts,
        "consumer.js",
    );
    assert!(
        output.contains(r#"import lib from "./provider.js""#),
        "{output}"
    );
}

#[test]
fn default_import_of_a_passthrough_to_a_default_with_named_provider_is_not_redirected() {
    // `module.exports = fn` beside named exports: the named exports are on
    // the namespace, not on `fn`, so neither import holds the old value.
    let facts = passthrough_facts_with_provider(ModuleFacts {
        commonjs_value_is_default: true,
        exports: vec![
            export("default", None, ExportKind::Default),
            export("extra", Some("extra"), ExportKind::Named),
        ],
        ..Default::default()
    });
    let output = consolidate_reexports(
        "import lib from \"./passthrough.js\";\nuse(lib.version);\n",
        &facts,
        "consumer.js",
    );
    assert!(
        output.contains(r#"import lib from "./passthrough.js""#),
        "{output}"
    );
}

#[test]
fn default_import_of_a_passthrough_to_a_node_builtin_is_redirected_to_its_namespace() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "passthrough.js",
        ModuleFacts {
            passthrough_target: Some("node:crypto".into()),
            ..Default::default()
        },
    );
    let output = consolidate_reexports(
        "import lib from \"./passthrough.js\";\nuse(lib.randomUUID());\n",
        &facts,
        "consumer.js",
    );
    assert!(
        output.contains(r#"import * as lib from "node:crypto""#),
        "{output}"
    );
}

#[test]
fn default_import_of_a_passthrough_to_a_marked_default_only_provider_stays_a_default_import() {
    // The provider's CommonJS value was its default, which also marked
    // itself `__esModule` (Next's default-object tail): a namespace import
    // would hold it only as `default`.
    let facts = passthrough_facts_with_provider(ModuleFacts {
        commonjs_value_is_default: true,
        marks_es_module: true,
        exports: vec![export("default", None, ExportKind::Default)],
        ..Default::default()
    });
    let output = consolidate_reexports(
        "import Link from \"./passthrough.js\";\nrender(Link);\n",
        &facts,
        "consumer.js",
    );
    assert!(
        output.contains(r#"import Link from "./provider.js""#),
        "{output}"
    );
}
