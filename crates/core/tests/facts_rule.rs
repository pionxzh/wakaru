mod common;

use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::facts::{
    collect_module_facts, ExportFact, ExportKind, HelperExportFact, HelperKind, ImportFact,
    ImportKind, ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact, TypeScriptHelperKind,
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

        // Run pipeline through end of Stage 2
        apply_rules(
            &mut module,
            unresolved_mark,
            RulePipelineOptions::until("UnEsm"),
        );

        collect_module_facts(&module)
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
