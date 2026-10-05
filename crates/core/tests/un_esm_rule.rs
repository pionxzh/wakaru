mod common;

use common::{
    assert_eq_normalized, render_pipeline, render_pipeline_until,
    render_pipeline_until_with_filename, render_pipeline_until_with_level,
};
use wakaru_core::{validate_output_modules, OutputFindingKind, RewriteLevel};

// Stop before DeadImports (the final cleanup pass) so that synthetic inputs
// with unused specifiers don't get stripped — these tests exercise UnEsm's
// shape, not downstream dead-code elimination.
fn apply(input: &str) -> String {
    render_pipeline_until(input, "SmartRename")
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_pipeline_until_with_level(input, "SmartRename", level)
}

#[test]
fn bare_require_to_import() {
    // require('side-effect') → import 'side-effect'
    let input = "require('side-effect');";
    let expected = r#"import "side-effect";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_does_not_convert_bare_require_to_import() {
    let input = "require('side-effect');";
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn local_require_binding_not_converted_to_import() {
    let input = r#"
function require(x) {
  return x;
}
var foo = require("foo");
"#;
    let output = render_pipeline_until(input, "UnEsm");
    assert_eq_normalized(&output, input);
}

#[test]
fn default_require_to_import() {
    let input = "var foo = require('foo');";
    let expected = r#"import foo from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn local_self_require_stays_at_the_commonjs_boundary() {
    let input = r#"
var self = require("./module-1.js");
for (var key in self) globalThis[key] = self[key];
"#;
    let expected = r#"
var self = require("./module-1.js");
for (var key in self) {
  globalThis[key] = self[key];
}
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, expected);
}

#[test]
fn local_self_require_keeps_the_complete_commonjs_surface() {
    let input = r#"
var self = require("./module-1.js");
var dependency = require("./module-2.js");
exports.value = dependency.value;
consume(self, dependency);
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, input);
    assert!(
        !output.contains("import ") && !output.contains("export "),
        "a self-requiring module must not cross only part of its CommonJS boundary:\n{output}"
    );
}

#[test]
fn linkable_default_self_import_keeps_existing_recovery() {
    let input = r#"
var self = require("./module-1.js");
module.exports = function api() {};
consume(self);
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert!(
        output.contains(r#"import self from "./module-1.js""#)
            && output.contains("export default")
            && output.contains("function api()")
            && !output.contains("require("),
        "a self-cycle with a real default surface should retain existing recovery:\n{output}"
    );
}

#[test]
fn named_self_surface_uses_the_proven_namespace_boundary() {
    let input = r#"
var self = require("./module-1.js");
exports.value = function value() {
  return self.value;
};
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert!(
        output.contains(r#"import * as self from "./module-1.js""#)
            && output.contains("export const value")
            && output.contains("return self.value")
            && !output.contains("require("),
        "a static read from a proven named self surface should use the existing namespace proof:\n{output}"
    );
}

#[test]
fn same_basename_in_another_directory_is_not_a_self_require() {
    let input = r#"
var sibling = require("../module-1.js");
consume(sibling);
"#;
    let expected = r#"
import sibling from "../module-1.js";
consume(sibling);
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "nested/module-1.js");

    assert_eq_normalized(&output, expected);
}

#[test]
fn shadowed_self_require_spelling_does_not_block_unesm() {
    let input = r#"
function inspect(require) {
  return require("./module-1.js");
}
var dependency = require("./module-2.js");
consume(inspect, dependency);
"#;
    let expected = r#"
import dependency from "./module-2.js";
function inspect(require) {
  return require("./module-1.js");
}
consume(inspect, dependency);
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, expected);
}

#[test]
fn multi_declarator_require_to_imports() {
    let input = r#"
var react = require("react"), jsx = require("react/jsx-runtime"), ctx = react.createContext(null);
"#;
    let expected = r#"
import react from "react";
import jsx from "react/jsx-runtime";
const ctx = react.createContext(null);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exported_require_to_import_and_export_specifier() {
    let input = r#"
export const dep = require("./dep.js");
export const value = dep.value;
"#;
    let expected = r#"
import dep from "./dep.js";
export { dep };
export const value = dep.value;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn mixed_exported_require_declaration_preserves_other_exports() {
    let input = r#"
export const local = 1, dep = require("./dep.js"), value = dep.value;
"#;
    let expected = r#"
import dep from "./dep.js";
export const local = 1;
export { dep };
export const value = dep.value;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn destructure_require_to_named_import() {
    // var { a, b: c } = require('foo')
    // UnEsm produces: import { a, b as c } from "foo"
    // UnImportRename then renames the alias `c` back to the imported name `b`
    let input = "var { a, b: c } = require('foo');";
    let expected = r#"import { a, b } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn property_require_to_named_import() {
    // UnEsm produces: import { baz as foo } from "bar"
    // UnImportRename then renames `foo` to `baz` (the imported name)
    let input = "var foo = require('bar').baz;";
    let expected = r#"import { baz } from "bar";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn default_property_require() {
    let input = "var foo = require('bar').default;";
    let expected = r#"import foo from "bar";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn webpack_default_getter_collapses_to_import() {
    let input = r#"
var r = require('foo');
var o = () => r && r.__esModule ? r.default : r;
function load() {
  return o();
}
"#;
    let expected = r#"
import r from "foo";
function load() {
  return r;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn merge_same_source_imports() {
    let input = r#"
var foo = require('foo');
var { bar } = require('foo');
"#;
    let expected = r#"import foo, { bar } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn commonjs_object_keys_reexport_loop_becomes_export_star() {
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  key !== "default" && key !== "__esModule" &&
    (key in exports && exports[key] === source[key] ||
      (exports[key] = source[key]));
});
"#;
    let expected = r#"export * from "./source.js";"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn commonjs_object_keys_reexport_loop_with_extra_binding_use_is_unchanged() {
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  key !== "default" && key !== "__esModule" &&
    (key in exports && exports[key] === source[key] ||
      (exports[key] = source[key]));
});
observe(source);
"#;
    let output = apply(input);
    assert!(
        !output.contains("export * from"),
        "an escaped require binding must keep the re-export loop:\n{output}"
    );
    assert!(output.contains("Object.keys(source)"));
}

#[test]
fn babel_export_star_with_export_names_and_getters_becomes_export_star() {
    // Babel's default (non-loose) output when the module also has its own
    // export: early-return guards, `_exportNames`, and live getters.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
var _exportNames = { local: true };
exports.local = void 0;
var _provider = require("./provider.js");
Object.keys(_provider).forEach(function (key) {
  if (key === "default" || key === "__esModule") return;
  if (Object.prototype.hasOwnProperty.call(_exportNames, key)) return;
  if (key in exports && exports[key] === _provider[key]) return;
  Object.defineProperty(exports, key, {
    enumerable: true,
    get: function get() {
      return _provider[key];
    }
  });
});
var local = exports.local = 1;
"#;
    let expected = r#"
"use strict";
export * from "./provider.js";
export const local = 1;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn minified_babel_export_star_with_export_names_becomes_export_star() {
    let input = r#"
"use strict";Object.defineProperty(exports,"__esModule",{value:!0});var _exportNames={local:!0};exports.local=void 0;var _provider=require("./provider.js");Object.keys(_provider).forEach(function(e){"default"!==e&&"__esModule"!==e&&(Object.prototype.hasOwnProperty.call(_exportNames,e)||e in exports&&exports[e]===_provider[e]||Object.defineProperty(exports,e,{enumerable:!0,get:function(){return _provider[e]}}))});var local=exports.local=1;
"#;
    let expected = r#"
"use strict";
export * from "./provider.js";
export const local = 1;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn babel_export_star_loop_in_or_chain_form_becomes_export_star() {
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  "default" === key || "__esModule" === key || key in exports && exports[key] === source[key] || (exports[key] = source[key]);
});
"#;
    assert_eq_normalized(&apply(input), r#"export * from "./source.js";"#);
}

#[test]
fn babel_export_star_loop_away_from_its_require_becomes_export_star() {
    // Terser merges adjacent declarations, so the require binding is not the
    // statement right before its loop.
    let input = r#"
var other = require("./other.js"), source = require("./source.js");
Object.keys(source).forEach(function(key) {
  "default" !== key && "__esModule" !== key && (key in exports && exports[key] === source[key] || (exports[key] = source[key]));
});
exports.run = function() { return other.value; };
"#;
    let output = apply(input);
    assert!(
        output.contains(r#"export * from "./source.js";"#),
        "{output}"
    );
    assert!(!output.contains("Object.keys"), "{output}");
    assert!(
        !output.contains("source.js\")"),
        "the consumed require must go:\n{output}"
    );
}

#[test]
fn export_star_loop_without_default_skip_is_unchanged() {
    // `export *` never re-exports `default`; a loop that copies it is not one.
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  "__esModule" !== key && (key in exports && exports[key] === source[key] || (exports[key] = source[key]));
});
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
    assert!(output.contains("Object.keys(source)"), "{output}");
}

#[test]
fn export_star_loop_that_can_overwrite_exports_is_unchanged() {
    // Without a skip of the module's own keys the loop overwrites the
    // module's own exports, which `export *` cannot do.
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  "default" !== key && (exports[key] = source[key]);
});
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
}

#[test]
fn export_star_loop_that_only_skips_es_module_needs_local_exports_after_it() {
    // An `__esModule` skip does not protect `x`: the loop overwrites it with
    // the source's `x`, while in ESM the local export shadows the star one.
    let input = r#"
var source = require("./source.js");
exports.x = 1;
Object.keys(source).forEach(function(key) {
  if (key === "default" || key === "__esModule") return;
  exports[key] = source[key];
});
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");

    // A write inside a function may run before the loop.
    let input = r#"
var source = require("./source.js");
init();
Object.keys(source).forEach(function(key) {
  if (key === "default" || key === "__esModule") return;
  exports[key] = source[key];
});
function init() { exports.x = 1; }
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");

    // A top-level write after the loop overwrites the copy, as the local
    // export shadows the star export in ESM.
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  if (key === "default" || key === "__esModule") return;
  exports[key] = source[key];
});
exports.x = 1;
"#;
    assert_eq_normalized(
        &apply(input),
        r#"export * from "./source.js"; export const x = 1;"#,
    );
}

#[test]
fn export_star_helper_that_only_skips_es_module_is_unchanged() {
    // A helper cannot see the exports at its call site, so it must skip keys
    // the target already owns.
    let input = r#"
var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (p !== "default" && p !== "__esModule") exports[p] = m[p];
};
exports.x = 1;
__exportStar(require("./provider.js"), exports);
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
}

#[test]
fn export_star_loop_with_extended_export_names_is_unchanged() {
    // `_exportNames.y = true` makes the loop skip `y`, which `export *`
    // would re-export.
    let input = r#"
var _exportNames = { x: true };
_exportNames.y = true;
exports.x = 1;
var _source = require("./source.js");
Object.keys(_source).forEach(function (key) {
  if (key === "default" || key === "__esModule") return;
  if (Object.prototype.hasOwnProperty.call(_exportNames, key)) return;
  if (key in exports && exports[key] === _source[key]) return;
  Object.defineProperty(exports, key, {
    enumerable: true,
    get: function () { return _source[key]; }
  });
});
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
}

#[test]
fn export_star_loop_with_extra_effect_is_unchanged() {
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  if (key === "default" || key === "__esModule") return;
  track(key);
  exports[key] = source[key];
});
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
    assert!(output.contains("track(key)"), "{output}");
}

#[test]
fn export_names_listing_a_name_the_module_does_not_export_is_unchanged() {
    // The loop skips `hidden`, but `export *` would re-export it.
    let input = r#"
var _exportNames = { hidden: true };
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  if (key === "default" || key === "__esModule") return;
  if (Object.prototype.hasOwnProperty.call(_exportNames, key)) return;
  exports[key] = source[key];
});
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
}

#[test]
fn export_star_loop_with_shadowed_object_is_unchanged() {
    let input = r#"
var Object = makeObject();
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  "default" !== key && "__esModule" !== key && (key in exports && exports[key] === source[key] || (exports[key] = source[key]));
});
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
}

#[test]
fn typescript_inline_export_star_helper_becomes_export_star() {
    let input = r#"
"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (p !== "default" && !Object.prototype.hasOwnProperty.call(exports, p)) __createBinding(exports, m, p);
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.local = void 0;
__exportStar(require("./provider.js"), exports);
__exportStar(require("./other.js"), exports);
exports.local = 1;
"#;
    let expected = r#"
"use strict";
export * from "./provider.js";
export * from "./other.js";
export const local = 1;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn minified_typescript_inline_export_star_helper_becomes_export_star() {
    let input = r#"
"use strict";var __createBinding=this&&this.__createBinding||(Object.create?function(e,t,r,o){void 0===o&&(o=r);var i=Object.getOwnPropertyDescriptor(t,r);i&&!("get"in i?!t.__esModule:i.writable||i.configurable)||(i={enumerable:!0,get:function(){return t[r]}}),Object.defineProperty(e,o,i)}:function(e,t,r,o){void 0===o&&(o=r),e[o]=t[r]}),__exportStar=this&&this.__exportStar||function(e,t){for(var r in e)"default"===r||Object.prototype.hasOwnProperty.call(t,r)||__createBinding(t,e,r)};Object.defineProperty(exports,"__esModule",{value:!0}),__exportStar(require("./provider.js"),exports);
"#;
    assert_eq_normalized(
        &apply(input),
        r#""use strict";
export * from "./provider.js";"#,
    );
}

#[test]
fn typescript_export_star_helper_with_unproven_body_is_unchanged() {
    // The `this.__exportStar` marker alone does not prove the helper: this
    // body copies `default` too.
    let input = r#"
var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (!Object.prototype.hasOwnProperty.call(exports, p)) exports[p] = m[p];
};
__exportStar(require("./provider.js"), exports);
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
    assert!(output.contains("__exportStar("), "{output}");
}

#[test]
fn export_star_helper_still_called_elsewhere_keeps_its_declaration() {
    let input = r#"
var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (p !== "default" && !Object.prototype.hasOwnProperty.call(exports, p)) exports[p] = m[p];
};
__exportStar(require("./provider.js"), exports);
__exportStar(require("./other.js"), target);
"#;
    let output = apply(input);
    assert!(
        output.contains(r#"export * from "./provider.js";"#),
        "{output}"
    );
    assert!(
        output.contains(r#"__exportStar(require("./other.js"), target)"#),
        "{output}"
    );
    assert!(output.contains("var __exportStar = "), "{output}");
}

#[test]
fn tslib_namespace_export_star_becomes_export_star() {
    let input = r#"
"use strict";Object.defineProperty(exports,"__esModule",{value:!0}),exports.local=void 0;const tslib_1=require("tslib");tslib_1.__exportStar(require("./provider.js"),exports),tslib_1.__exportStar(require("./other.js"),exports),exports.local=1;
"#;
    let output = apply(input);
    assert!(
        output.contains(r#"export * from "./provider.js";"#),
        "{output}"
    );
    assert!(
        output.contains(r#"export * from "./other.js";"#),
        "{output}"
    );
    assert!(!output.contains("__exportStar"), "{output}");
    assert!(!output.contains("exports"), "{output}");
}

#[test]
fn export_star_helper_call_on_module_exports_is_unchanged() {
    let input = r#"
const tslib_1 = require("tslib");
tslib_1.__exportStar(require("./provider.js"), module.exports);
"#;
    let output = apply(input);
    assert!(!output.contains("export * from"), "{output}");
}

#[test]
fn swc_external_export_star_helper_becomes_export_star() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", {
    value: true
});
Object.defineProperty(exports, "local", {
    enumerable: true,
    get: function() {
        return local;
    }
});
const _export_star = require("@swc/helpers/_/_export_star");
_export_star._(require("./provider.js"), exports);
_export_star._(require("./other.js"), exports);
const local = 1;
"#;
    let output = apply(input);
    assert!(
        output.contains(r#"export * from "./provider.js";"#),
        "{output}"
    );
    assert!(
        output.contains(r#"export * from "./other.js";"#),
        "{output}"
    );
    assert!(!output.contains("_export_star"), "{output}");
}

#[test]
fn swc_export_star_helper_from_another_path_is_unchanged() {
    let input = r#"
const _export_star = require("./export-star.js");
_export_star._(require("./provider.js"), exports);
"#;
    let output = apply(input);
    assert!(
        !output.contains("export * from \"./provider.js\""),
        "{output}"
    );
}

#[test]
fn minified_swc_inline_export_star_helper_becomes_export_star() {
    let input = r#"
"use strict";function _export_star(e,r){return Object.keys(e).forEach(function(t){"default"===t||Object.prototype.hasOwnProperty.call(r,t)||Object.defineProperty(r,t,{enumerable:!0,get:function(){return e[t]}})}),e}Object.defineProperty(exports,"__esModule",{value:!0}),_export_star(require("./provider.js"),exports);
"#;
    assert_eq_normalized(
        &apply(input),
        r#""use strict";
export * from "./provider.js";"#,
    );
}

#[test]
fn swc_inline_export_star_helper_becomes_export_star() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
_export_star(require("./provider.js"), exports);
function _export_star(from, to) {
    Object.keys(from).forEach(function(k) {
        if (k !== "default" && !Object.prototype.hasOwnProperty.call(to, k)) {
            Object.defineProperty(to, k, {
                enumerable: true,
                get: function() {
                    return from[k];
                }
            });
        }
    });
    return from;
}
"#;
    assert_eq_normalized(
        &apply(input),
        r#""use strict";
export * from "./provider.js";"#,
    );
}

#[test]
fn multiple_defaults_separate_imports() {
    // Two require() calls for the same module produce the same value;
    // ImportDedup canonicalizes to the first local binding.
    let input = r#"
var foo = require('foo');
var bar = require('foo');
"#;
    let expected = r#"
import foo from "foo";
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn mutable_require_binding_uses_a_separate_import_binding() {
    let input = r#"
var dependency = require("./dependency.js");
dependency = replacement;
consume(dependency);
"#;
    let expected = r#"
import _dependency from "./dependency.js";
let dependency = _dependency;
dependency = replacement;
consume(dependency);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);

    let findings = validate_output_modules(&[
        ("entry.js".to_string(), output),
        (
            "dependency.js".to_string(),
            "export default {};".to_string(),
        ),
    ]);
    assert!(
        !findings
            .iter()
            .any(|finding| finding.kind == OutputFindingKind::AssignToImport),
        "mutable local must not write to the synthesized import: {findings:#?}"
    );
}

#[test]
fn written_const_require_binding_preserves_its_authored_contract() {
    let input = r#"
const dependency = require("./dependency.js");
dependency = replacement;
consume(dependency);
"#;
    let expected = r#"
import _dependency from "./dependency.js";
const dependency = _dependency;
dependency = replacement;
consume(dependency);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn redeclared_require_binding_uses_a_separate_import_binding() {
    let input = r#"
var dependency = require("./dependency.js");
var dependency = replacement;
exports.value = dependency.make();
"#;
    let output = apply(input);
    assert!(output.contains("import _dependency from"), "{output}");
    assert!(
        validate_output_modules(&[
            ("entry.js".to_string(), output.clone()),
            (
                "dependency.js".to_string(),
                "export default {};".to_string()
            ),
        ])
        .is_empty(),
        "an import cannot share its name with a redeclared var:\n{output}"
    );
}

#[test]
fn nested_write_to_require_binding_stays_on_the_local() {
    let input = r#"
var dependency = require("./dependency.js");
function replaceDependency(next) {
    dependency = next;
}
consume(dependency, replaceDependency);
"#;
    let expected = r#"
import _dependency from "./dependency.js";
let dependency = _dependency;
function replaceDependency(next) {
    dependency = next;
}
consume(dependency, replaceDependency);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn update_to_require_binding_stays_on_the_local() {
    let input = r#"
var counter = require("./counter.js");
counter++;
consume(counter);
"#;
    let expected = r#"
import _counter from "./counter.js";
let counter = _counter;
counter++;
consume(counter);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn mutable_destructured_require_uses_a_separate_import_binding() {
    let input = r#"
var { value } = require("./dependency.js");
value = replacement;
consume(value);
"#;
    let expected = r#"
import _value from "./dependency.js";
let { value } = _value;
value = replacement;
consume(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn mutable_named_property_require_uses_a_separate_import_binding() {
    let input = r#"
var value = require("./dependency.js").value;
value = replacement;
consume(value);
"#;
    let expected = r#"
import { value as value_1 } from "./dependency.js";
let value = value_1;
value = replacement;
consume(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn mutable_default_property_require_uses_a_separate_import_binding() {
    let input = r#"
var value = require("./dependency.js").default;
value = replacement;
consume(value);
"#;
    let expected = r#"
import _value from "./dependency.js";
let value = _value;
value = replacement;
consume(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn exported_mutable_require_retains_the_local_export() {
    let input = r#"
export let dependency = require("./dependency.js");
dependency = replacement;
"#;
    let expected = r#"
import _dependency from "./dependency.js";
export let dependency = _dependency;
dependency = replacement;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn multiple_mutable_requires_share_the_canonical_import() {
    let input = r#"
var first = require("./dependency.js");
var second = require("./dependency.js");
first = replacementOne;
second = replacementTwo;
consume(first, second);
"#;
    let expected = r#"
import _first from "./dependency.js";
let first = _first;
let second = _first;
first = replacementOne;
second = replacementTwo;
consume(first, second);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn module_exports_default() {
    let input = "module.exports = 1;";
    let expected = "export default 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn called_module_exports_assignment_preserves_export_and_call() {
    let input = r#"(module.exports = factory)(argument);"#;
    let expected = r#"
const _default = factory;
export default _default;
_default(argument);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn called_module_exports_assignment_evaluates_rhs_once() {
    let input = r#"(module.exports = createFactory())(argument);"#;
    let expected = r#"
const _default = createFactory();
export default _default;
_default(argument);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn called_module_exports_assignment_as_member_receiver_preserves_export_and_call() {
    let input = r#"
(module.exports = factory)("versions", []).push(record);
"#;
    let expected = r#"
const _default = factory;
export default _default;
_default("versions", []).push(record);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn called_module_exports_assignment_receiver_chain_evaluates_rhs_once() {
    let input = r#"
(module.exports = createFactory())(argument).result.consume();
"#;
    let expected = r#"
const _default = createFactory();
export default _default;
_default(argument).result.consume();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn called_local_module_exports_assignment_is_not_transformed() {
    let input = r#"
const module = { exports: null };
(module.exports = factory)(argument);
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn module_exports_assignment_in_single_var_initializer_preserves_binding() {
    let input = r#"
var value = module.exports = createValue();
use(value);
"#;
    let expected = r#"
const value = createValue();
export default value;
use(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn module_exports_assignment_in_single_var_initializer_evaluates_rhs_once() {
    let input = r#"
var value = module.exports = makeValue(sideEffect());
consume(value);
"#;
    let output = apply(input);
    assert_eq!(output.matches("makeValue(sideEffect())").count(), 1);
    assert!(output.contains("export default value;"));
}

#[test]
fn local_module_exports_assignment_in_var_initializer_is_not_transformed() {
    let input = r#"
const module = { exports: null };
var value = module.exports = createValue();
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn module_exports_assignment_in_split_multi_var_initializer_preserves_order() {
    let input = r#"
var before = observeBefore(), value = module.exports = createValue(), after = observeAfter();
"#;
    let expected = r#"
var before = observeBefore();
var value = createValue();
export default value;
var after = observeAfter();
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), expected);
}

#[test]
fn chained_local_module_exports_assignment_preserves_order() {
    let input = r#"
let value;
value = module.exports = createValue();
consume(value);
"#;
    let expected = r#"
let value;
const _default = createValue();
export default _default;
value = _default;
consume(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn chained_local_module_exports_assignment_evaluates_rhs_once() {
    let input = r#"
let value;
value = module.exports = makeValue(sideEffect());
"#;
    let output = apply(input);
    assert_eq!(output.matches("makeValue(sideEffect())").count(), 1);
    assert!(output.contains("export default _default;"));
    assert!(output.contains("value = _default;"));
}

#[test]
fn chained_local_module_exports_assignment_with_local_module_is_not_transformed() {
    let input = r#"
const module = { exports: null };
let value;
value = module.exports = createValue();
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn chained_unresolved_module_exports_assignment_is_not_transformed() {
    let input = r#"value = module.exports = createValue();"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn chained_const_module_exports_assignment_is_not_transformed() {
    let input = r#"
const value = initialValue;
value = module.exports = createValue();
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn chained_member_module_exports_assignment_is_not_transformed() {
    let input = r#"holder.value = module.exports = createValue();"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn module_exports_default_ident_not_affected() {
    // CJS module.exports = ident still produces export default (the declaration
    // is before the export, so no TDZ issue).
    let input = r#"
const o = { foo: 1 };
module.exports = o;
"#;
    let expected = r#"
const o = { foo: 1 };
export default o;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn stable_default_binding_replaces_later_commonjs_mirror_reads() {
    let input = r#"
const api = () => "ready";
module.exports = api;
if (typeof window !== "undefined") {
  window.syntheticApi = module.exports;
}
"#;
    let expected = r#"
const api = () => "ready";
export default api;
if (typeof window !== "undefined") {
  window.syntheticApi = api;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    assert!(validate_output_modules(&[("entry.js".into(), output)])
        .iter()
        .all(|finding| finding.kind != OutputFindingKind::EsmCommonJsResidual));
}

#[test]
fn stable_default_read_recovery_rejects_a_second_assignment() {
    let input = r#"
const api = () => "ready";
module.exports = api;
if (legacy) module.exports = replacement;
window.syntheticApi = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("window.syntheticApi = module.exports"),
        "a conditional second value must keep default reads fail closed:\n{output}"
    );
}

#[test]
fn stable_default_read_recovery_rejects_a_reassigned_capture() {
    let input = r#"
let api = () => "ready";
module.exports = api;
api = replacement;
window.syntheticApi = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("window.syntheticApi = module.exports"),
        "a mutable capture does not prove the later CommonJS value:\n{output}"
    );
}

#[test]
fn stable_default_read_recovery_rejects_hidden_direct_eval_writes() {
    let input = r#"
let api = () => "ready";
module.exports = api;
eval("api = replacement");
window.syntheticApi = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("window.syntheticApi = module.exports"),
        "direct eval can replace a capture without an AST write site:\n{output}"
    );
}

#[test]
fn stable_default_read_recovery_preserves_direct_calls() {
    let input = r#"
const api = function() { return this.value; };
module.exports = api;
consume(module.exports());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(module.exports())"),
        "a direct CommonJS call supplies module as its receiver:\n{output}"
    );
}

#[test]
fn exports_named_const() {
    let input = "exports.foo = 1;";
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn stable_named_function_exports_replace_later_self_reads() {
    let input = r#"
Object.defineProperty(exports, "__esModule", { value: true }),
  exports.second = exports.first = void 0;
var helper = (
  exports.first = function(value) { return value + 1; },
  exports.second = function(value) { return exports.first(value); },
  consume(exports.second(1)),
  function(value) { return value; }
);
"#;
    let expected = r#"
export const first = function(value) {
    return value + 1;
};
export const second = function(value) {
    return first(value);
};
consume(second(1));
const helper = value => value;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    assert!(validate_output_modules(&[("entry.js".into(), output)])
        .iter()
        .all(|finding| finding.kind != OutputFindingKind::EsmCommonJsResidual));
}

#[test]
fn named_export_read_recovery_preserves_direct_eval_receiver_reads() {
    let input = r#"
exports.method = function() { return eval("this.value"); };
consume(exports.method());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method())"),
        "direct eval can observe the CommonJS receiver:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_rejects_hidden_direct_eval_property_writes() {
    let input = r#"
exports.method = () => 1;
eval("exports.method = replacement");
consume(exports.method());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method())"),
        "direct eval can replace the CommonJS property without an AST write site:\n{output}"
    );
}

#[test]
fn stable_named_object_exports_replace_later_member_reads() {
    let input = r#"
exports.events = createEvents();
exports.notify = () => exports.events.dispatch("ready");
"#;
    let expected = r#"
export const events = createEvents();
export const notify = () => events.dispatch("ready");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn property_storage_keeps_a_later_reset() {
    let input = r#"
exports.method = () => 1;
exports.method = void 0;
consume(exports.method());
"#;
    let expected = r#"
export let method = () => 1;
method = undefined;
consume(method());
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn named_export_read_recovery_preserves_receiver_dependent_functions() {
    let input = r#"
exports.method = function() { return this.value; };
consume(exports.method());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method())"),
        "ordinary functions still observe the CommonJS receiver:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_preserves_receiver_dependent_optional_calls_and_tags() {
    let input = r#"
exports.method = function() { return this.value; };
consume(exports.method?.());
consume(exports.method`value`);
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method?.())")
            && output.contains("consume(exports.method`value`)"),
        "optional calls and tags also supply the CommonJS receiver:\n{output}"
    );
}

#[test]
fn property_storage_follows_repeated_writes() {
    let input = r#"
exports.first = () => 1;
exports.first = () => 2;
exports.second = () => exports.first();
"#;
    let expected = r#"
export let first = () => 1;
first = () => 2;
export const second = () => first();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn named_export_read_recovery_rejects_computed_exports_mutation() {
    let input = r#"
exports.first = () => 1;
exports[key] = replacement;
consume(exports.first());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.first())"),
        "a computed mutation can alias any recovered property:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_rejects_prototype_mutating_member_uses() {
    let getter_installer = r#"
exports.first = () => 1;
exports.__defineGetter__("first", () => () => 2);
consume(exports.first());
"#;
    let output = apply(getter_installer);
    assert!(
        output.contains("consume(exports.first())"),
        "a legacy getter installer can redefine any proven property:\n{output}"
    );

    let prototype_write = r#"
exports.first = () => 1;
exports.__proto__ = fallback;
consume(exports.first());
"#;
    let output = apply(prototype_write);
    assert!(
        output.contains("consume(exports.first())"),
        "a prototype write changes lookup without a visible property write:\n{output}"
    );
}

#[test]
fn property_storage_reads_before_the_write_see_the_hoisted_var() {
    // The hoisted `var` is undefined at the earlier read, like the property.
    let input = r#"
consume(exports.first);
exports.first = () => 1;
"#;
    let expected = r#"
consume(first);
export var first = () => 1;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn property_storage_rewrites_hoisted_function_declarations() {
    let input = r#"
invoke();
exports.first = () => 1;
function invoke() { return exports.first(); }
"#;
    let expected = r#"
invoke();
export var first = () => 1;
function invoke() {
  return first();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn local_exports_binding_not_converted_to_export() {
    let input = r#"
var exports = {};
exports.foo = 1;
"#;
    let output = render_pipeline_until(input, "UnEsm");
    assert_eq_normalized(&output, input);
}

#[test]
fn esmodule_marker_on_arbitrary_object_does_not_create_exports_alias() {
    let input = r#"
Object.defineProperty(moduleExports, "__esModule", { value: true });
moduleExports.Service = void 0;
class Service {}
moduleExports.Service = Service;
"#;
    let expected = r#"
Object.defineProperty(moduleExports, "__esModule", {
    value: true
});
moduleExports.Service = undefined;
class Service {}
moduleExports.Service = Service;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn define_property_getter_on_exports_to_named_export() {
    let input = r#"
const rawCache = require("./raw-cache.js");
Object.defineProperty(exports, "rawCache", {
  enumerable: true,
  get() {
    return rawCache;
  }
});
"#;
    let expected = r#"
import rawCache from "./raw-cache.js";
export { rawCache };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn define_property_getter_to_mutable_binding_stays_live() {
    let input = r#"
let value = 1;
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return value;
  }
});
value = 2;
"#;
    let expected = r#"
export let value = 1;
value = 2;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn define_property_member_getter_becomes_live_reexport() {
    let input = r#"
const dep = require("./dep.js");
Object.defineProperty(exports, "renamed", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let expected = r#"
export { value as renamed } from "./dep.js";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn define_property_member_arrow_getter_becomes_live_reexport() {
    let input = r#"
var dep = require("./dep.js");
Object.defineProperty(exports, "value", {
  enumerable: true,
  get: () => dep.value
});
"#;
    let expected = r#"
export { value } from "./dep.js";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn define_property_member_getter_supports_default_reexport() {
    let input = r#"
const dep = require("./dep.js");
Object.defineProperty(exports, "default", {
  enumerable: true,
  get: () => dep.value
});
"#;
    let expected = r#"
export { value as default } from "./dep.js";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn live_reexport_retains_import_when_require_binding_has_other_reads() {
    let input = r#"
const dep = require("./dep.js");
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
consume(dep.other);
"#;
    let expected = r#"
import dep from "./dep.js";
export { value } from "./dep.js";
consume(dep.other);
"#;
    assert_eq_normalized(&apply(input), expected);
}

const BABEL_INTEROP_REQUIRE_WILDCARD: &str = r#"
function _interopRequireWildcard(e, t) { if ("function" == typeof WeakMap) var r = new WeakMap(), n = new WeakMap(); return (_interopRequireWildcard = function (e, t) { if (!t && e && e.__esModule) return e; var o, i, f = { __proto__: null, default: e }; if (null === e || "object" != typeof e && "function" != typeof e) return f; if (o = t ? n : r) { if (o.has(e)) return o.get(e); o.set(e, f); } for (const t in e) "default" !== t && {}.hasOwnProperty.call(e, t) && ((i = (o = Object.defineProperty) && Object.getOwnPropertyDescriptor(e, t)) && (i.get || i.set) ? o(f, t, i) : f[t] = e[t]); return f; })(e, t); }
"#;

#[test]
fn define_property_getter_of_wildcard_import_becomes_live_reexport() {
    // Babel defines the getters before the wildcard require, which
    // `UnInteropRequireWildcard` has turned into a namespace import by the
    // time `UnEsm` runs.
    let input = format!(
        r#"
Object.defineProperty(exports, "__esModule", {{ value: true }});
Object.defineProperty(exports, "count", {{
  enumerable: true,
  get: function () {{
    return _dep.count;
  }}
}});
Object.defineProperty(exports, "depDefault", {{
  enumerable: true,
  get: function () {{
    return _dep.default;
  }}
}});
var _dep = _interopRequireWildcard(require("./dep.js"));
{BABEL_INTEROP_REQUIRE_WILDCARD}
"#
    );
    let expected = r#"
export { count } from "./dep.js";
export { default as depDefault } from "./dep.js";
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn live_reexport_of_wildcard_import_retains_import_with_other_reads() {
    let input = format!(
        r#"
Object.defineProperty(exports, "count", {{
  enumerable: true,
  get: function () {{
    return _dep.count;
  }}
}});
var _dep = _interopRequireWildcard(require("./dep.js"));
consume(_dep.other);
{BABEL_INTEROP_REQUIRE_WILDCARD}
"#
    );
    let expected = r#"
import * as _dep from "./dep.js";
export { count } from "./dep.js";
consume(_dep.other);
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn define_property_member_getter_rejects_reassigned_require_binding() {
    let input = r#"
let dep = require("./dep.js");
dep = replacement;
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let output = apply(input);
    assert!(output.contains("Object.defineProperty(exports, \"value\""));
    assert!(!output.contains("export { value } from"));
}

const SWC_EXPORT_HELPER: &str = r#"
function _export(target, all) {
    for(var name in all)Object.defineProperty(target, name, {
        enumerable: true,
        get: Object.getOwnPropertyDescriptor(all, name).get
    });
}
"#;

const ESBUILD_CJS_HELPERS: &str = r#"
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);
"#;

#[test]
fn swc_export_helper_getters_become_live_exports() {
    let input = format!(
        r#"
Object.defineProperty(exports, "__esModule", {{ value: true }});
{SWC_EXPORT_HELPER}
_export(exports, {{
    get bump () {{
        return bump;
    }},
    get count () {{
        return count;
    }},
    get default () {{
        return main;
    }}
}});
let count = 0;
function bump() {{
    count += 1;
}}
function main() {{
    return count;
}}
"#
    );
    let expected = r#"
export let count = 0;
function bump() {
    count += 1;
}
export { bump };
function main() {
    return count;
}
export { main as default };
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn older_swc_export_helper_with_function_values_becomes_live_exports() {
    let input = r#"
function _export(target, all) {
    for(var name in all)Object.defineProperty(target, name, {
        enumerable: true,
        get: all[name]
    });
}
_export(exports, {
    count: function() {
        return count;
    },
    bump: function() {
        return bump;
    }
});
var count = 0;
function bump() {
    count += 1;
}
"#;
    let expected = r#"
export let count = 0;
function bump() {
    count += 1;
}
export { bump };
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn getter_helper_keeps_entries_its_getter_read_does_not_expect() {
    // The accessor-reading helper would define `get: undefined` for a value
    // entry; the call stays as written.
    let input = format!(
        r#"
{SWC_EXPORT_HELPER}
_export(exports, {{
    count: function() {{
        return count;
    }}
}});
var count = 0;
"#
    );
    let output = apply(&input);
    assert!(output.contains("_export(exports"), "{output}");
}

#[test]
fn getter_helper_keeps_a_map_with_a_repeated_name() {
    // Defining the same non-configurable property twice throws.
    let input = r#"
function _export(target, all) {
    for(var name in all)Object.defineProperty(target, name, {
        enumerable: true,
        get: all[name]
    });
}
_export(exports, {
    count: () => count,
    count: () => other
});
var count = 0;
var other = 1;
"#;
    let output = apply(input);
    assert!(output.contains("_export(exports"), "{output}");
}

#[test]
fn getter_helper_string_name_becomes_quoted_live_export() {
    let input = format!(
        r#"
{SWC_EXPORT_HELPER}
_export(exports, {{
    get "a-b" () {{
        return v;
    }}
}});
var v = 1;
function bump() {{
    v++;
}}
exports.bump = bump;
"#
    );
    let output = apply(&input);
    assert!(!output.contains("exports"), "{output}");
    assert!(output.contains(r#"v as "a-b""#), "{output}");
}

#[test]
fn esbuild_to_common_js_namespace_becomes_live_exports() {
    let input = format!(
        r#"
{ESBUILD_CJS_HELPERS}
var mod_exports = {{}};
__export(mod_exports, {{
  count: () => count,
  default: () => mod_default,
  reset: () => reset
}});
module.exports = __toCommonJS(mod_exports);
let count = 0;
function reset() {{
  count = 0;
}}
var mod_default = count;
"#
    );
    let output = apply(&input);
    for helper in [
        "__export",
        "__toCommonJS",
        "__copyProps",
        "__hasOwnProp",
        "mod_exports",
    ] {
        assert!(!output.contains(helper), "{helper} left in {output}");
    }
    assert!(!output.contains("module.exports"), "{output}");
    assert!(output.contains("export let count = 0"), "{output}");
    assert!(output.contains("export { reset }"), "{output}");
}

#[test]
fn esbuild_to_common_js_namespace_stays_when_module_reads_exports() {
    // Getters on `exports` would differ from getters on the replaced
    // `module.exports` once the module refers to `exports` itself.
    let input = format!(
        r#"
{ESBUILD_CJS_HELPERS}
var mod_exports = {{}};
__export(mod_exports, {{
  count: () => count
}});
module.exports = __toCommonJS(mod_exports);
let count = 0;
exports.extra = 1;
"#
    );
    let output = apply(&input);
    assert!(output.contains("__toCommonJS(mod_exports)"), "{output}");
}

#[test]
fn sucrase_named_export_from_becomes_reexport() {
    let input = r#"
Object.defineProperty(exports, "__esModule", {value: true}); function _createNamedExportFrom(obj, localName, importedName) { Object.defineProperty(exports, localName, {enumerable: true, configurable: true, get: () => obj[importedName]}); }
var _depjs = require('./dep.js'); _createNamedExportFrom(_depjs, 'depCount', 'depCount'); _createNamedExportFrom(_depjs, 'depDefault', 'default'); _createNamedExportFrom(_depjs, 'renamed', 'depCount');
"#;
    let expected = r#"
export { depCount } from "./dep.js";
export { default as depDefault } from "./dep.js";
export { depCount as renamed } from "./dep.js";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sucrase_named_export_from_keeps_call_on_written_source() {
    let input = r#"
function _createNamedExportFrom(obj, localName, importedName) { Object.defineProperty(exports, localName, {enumerable: true, configurable: true, get: () => obj[importedName]}); }
var _depjs = require('./dep.js'); _createNamedExportFrom(_depjs, 'depCount', 'depCount');
_depjs = other;
"#;
    let output = apply(input);
    assert!(output.contains("_createNamedExportFrom(_depjs"), "{output}");
}

const SWC_INTEROP_REQUIRE_WILDCARD: &str = r#"
function _getRequireWildcardCache(nodeInterop) {
    if (typeof WeakMap !== "function") return null;
    var cacheBabelInterop = new WeakMap();
    var cacheNodeInterop = new WeakMap();
    return (_getRequireWildcardCache = function(nodeInterop) {
        return nodeInterop ? cacheNodeInterop : cacheBabelInterop;
    })(nodeInterop);
}
function _interop_require_wildcard(obj, nodeInterop) {
    if (!nodeInterop && obj && obj.__esModule) return obj;
    if (obj === null || typeof obj !== "object" && typeof obj !== "function") return {
        default: obj
    };
    var cache = _getRequireWildcardCache(nodeInterop);
    if (cache && cache.has(obj)) return cache.get(obj);
    var newObj = {
        __proto__: null
    };
    var hasPropertyDescriptor = Object.defineProperty && Object.getOwnPropertyDescriptor;
    for(var key in obj){
        if (key !== "default" && Object.prototype.hasOwnProperty.call(obj, key)) {
            var desc = hasPropertyDescriptor ? Object.getOwnPropertyDescriptor(obj, key) : null;
            if (desc && (desc.get || desc.set)) Object.defineProperty(newObj, key, desc);
            else newObj[key] = obj[key];
        }
    }
    newObj.default = obj;
    if (cache) cache.set(obj, newObj);
    return newObj;
}
"#;

#[test]
fn swc_star_and_namespace_reexport_of_one_source_are_recovered() {
    let input = format!(
        r#"
Object.defineProperty(exports, "__esModule", {{ value: true }});
{SWC_EXPORT_HELPER}
_export(exports, {{
    get ns () {{
        return _dep;
    }},
    get own () {{
        return own;
    }}
}});
var _dep = /*#__PURE__*/ _interop_require_wildcard(_export_star(require("./dep.js"), exports));
function _export_star(from, to) {{
    Object.keys(from).forEach(function(k) {{
        if (k !== "default" && !Object.prototype.hasOwnProperty.call(to, k)) {{
            Object.defineProperty(to, k, {{
                enumerable: true,
                get: function() {{
                    return from[k];
                }}
            }});
        }}
    }});
    return from;
}}
{SWC_INTEROP_REQUIRE_WILDCARD}
var own = 1;
"#
    );
    let output = apply(&input);
    assert!(!output.contains("require"), "{output}");
    assert!(!output.contains("exports"), "{output}");
    assert!(output.contains(r#"export * from "./dep.js";"#), "{output}");
    assert!(
        output.contains(r#"import * as _dep from "./dep.js";"#),
        "{output}"
    );
    assert!(output.contains("_dep as ns"), "{output}");
}

#[test]
fn namespace_export_star_needs_a_helper_that_returns_its_source() {
    // tslib's `__exportStar` returns nothing; the namespace would wrap
    // `undefined`.
    let input = r#"
var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (p !== "default" && !Object.prototype.hasOwnProperty.call(exports, p)) exports[p] = m[p];
};
var tslib_1 = require("tslib");
var _dep = tslib_1.__importStar(__exportStar(require("./dep.js"), exports));
consume(_dep);
"#;
    let output = apply(input);
    assert!(!output.contains("export *"), "{output}");
}

const ESBUILD_ESM_INTEROP_HELPERS: &str = r#"
var __create = Object.create;
var __getProtoOf = Object.getPrototypeOf;
var __reExport = (target, mod, secondTarget) => (__copyProps(target, mod, "default"), secondTarget && __copyProps(secondTarget, mod, "default"));
var __toESM = (mod, isNodeMode, target) => (target = mod != null ? __create(__getProtoOf(mod)) : {}, __copyProps(
  isNodeMode || !mod || !mod.__esModule ? __defProp(target, "default", { value: mod, enumerable: true }) : target,
  mod
));
"#;

#[test]
fn esbuild_star_and_namespace_reexports_are_recovered() {
    let input = format!(
        r#"
{ESBUILD_CJS_HELPERS}
{ESBUILD_ESM_INTEROP_HELPERS}
var mod_exports = {{}};
__export(mod_exports, {{
  ns: () => ns,
  own: () => own
}});
module.exports = __toCommonJS(mod_exports);
__reExport(mod_exports, require("./dep.js"), module.exports);
var ns = __toESM(require("./dep.js"));
const own = 1;
"#
    );
    let output = apply(&input);
    for leftover in ["require", "exports", "__reExport", "__toESM"] {
        assert!(!output.contains(leftover), "{leftover} left in {output}");
    }
    assert!(output.contains(r#"export * from "./dep.js";"#), "{output}");
    assert!(
        output.contains(r#"import * as ns from "./dep.js";"#),
        "{output}"
    );
    assert!(output.contains("export { ns }"), "{output}");
}

#[test]
fn esbuild_to_esm_in_node_mode_stays() {
    // `isNodeMode` makes `default` the whole module even for a module marked
    // `__esModule`, which a namespace import would not.
    let input = format!(
        r#"
{ESBUILD_CJS_HELPERS}
{ESBUILD_ESM_INTEROP_HELPERS}
var import_dep = __toESM(require("./dep.js"), 1);
console.log(import_dep.default);
"#
    );
    let output = apply(&input);
    assert!(output.contains("__toESM("), "{output}");
}

#[test]
fn sucrase_star_and_namespace_reexports_are_recovered() {
    let input = r#"
"use strict";Object.defineProperty(exports, "__esModule", {value: true}); function _interopRequireWildcard(obj) { if (obj && obj.__esModule) { return obj; } else { var newObj = {}; if (obj != null) { for (var key in obj) { if (Object.prototype.hasOwnProperty.call(obj, key)) { newObj[key] = obj[key]; } } } newObj.default = obj; return newObj; } } function _createStarExport(obj) { Object.keys(obj) .filter((key) => key !== "default" && key !== "__esModule") .forEach((key) => { if (exports.hasOwnProperty(key)) { return; } Object.defineProperty(exports, key, {enumerable: true, configurable: true, get: () => obj[key]}); }); }
var _depjs = require('./dep.js'); var _depjs2 = _interopRequireWildcard(_depjs); exports.ns = _depjs2; _createStarExport(_depjs);

 const own = 1; exports.own = own;
"#;
    let output = apply(input);
    for leftover in [
        "require",
        "exports",
        "_createStarExport",
        "_interopRequireWildcard(",
    ] {
        assert!(!output.contains(leftover), "{leftover} left in {output}");
    }
    assert!(output.contains(r#"export * from "./dep.js";"#), "{output}");
    assert!(output.contains("import * as _depjs2 from"), "{output}");
}

#[test]
fn source_only_star_helper_must_skip_default() {
    // Without the `default` filter the copy would re-export `default`,
    // which `export *` never does.
    let input = r#"
function _createStarExport(obj) { Object.keys(obj) .filter((key) => key !== "__esModule") .forEach((key) => { if (exports.hasOwnProperty(key)) { return; } Object.defineProperty(exports, key, {enumerable: true, configurable: true, get: () => obj[key]}); }); }
var _depjs = require('./dep.js'); _createStarExport(_depjs);
"#;
    let output = apply(input);
    assert!(!output.contains("export *"), "{output}");
}

#[test]
fn define_property_member_getter_rejects_member_writes() {
    let input = r#"
const dep = require("./dep.js");
dep.value = replacement;
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let output = apply(input);
    assert!(output.contains("Object.defineProperty(exports, \"value\""));
    assert!(!output.contains("export { value } from"));
}

#[test]
fn define_property_member_getter_of_escaped_require_stays_live_reexport() {
    let input = r#"
const dep = require("./dep.js");
consume(dep);
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let expected = r#"
import dep from "./dep.js";
consume(dep);
export { value } from "./dep.js";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn define_property_member_getter_rejects_member_delete() {
    let input = r#"
const dep = require("./dep.js");
delete dep.value;
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let output = apply(input);
    assert!(output.contains("Object.defineProperty(exports, \"value\""));
    assert!(!output.contains("export { value } from"));
}

#[test]
fn define_property_member_getter_rejects_dynamic_property() {
    let input = r#"
const dep = require("./dep.js");
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep[key];
  }
});
"#;
    let output = apply(input);
    assert!(output.contains("Object.defineProperty(exports, \"value\""));
    assert!(!output.contains("export { value } from"));
}

#[test]
fn define_property_getter_on_arbitrary_object_is_not_export() {
    let input = r#"
Object.defineProperty(moduleExports, "__esModule", {
  value: true
});
Object.defineProperty(moduleExports, "helperValue", {
  enumerable: true,
  get() {
    return helperValue;
  }
});
const helperValue = createHelperValue();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn define_property_default_getter_uses_live_export_specifier() {
    let input = r#"
const value = createValue();
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() {
    return value;
  }
});
"#;
    let expected = r#"
const value = createValue();
export { value as default };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn define_property_getter_with_call_return_is_not_export() {
    let input = r#"
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return compute();
  }
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn define_property_getter_with_unresolved_return_is_not_export() {
    let input = r#"
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return globalValue;
  }
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn define_property_getter_with_effectful_descriptor_is_not_export() {
    let input = r#"
const value = createValue();
Object.defineProperty(exports, "value", {
  enumerable: computeEnumerable(),
  get() {
    return value;
  }
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn local_module_binding_not_converted_to_export() {
    let input = r#"
var module = { exports: {} };
module.exports = value;
"#;
    let output = render_pipeline_until(input, "UnEsm");
    assert_eq_normalized(&output, input);
}

#[test]
fn exports_named_same_ident() {
    let input = r#"
function foo() {}
exports.foo = foo;
"#;
    let expected = r#"
function foo() {}
export { foo };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_default_prop() {
    let input = "exports.default = 42;";
    let expected = "export default 42;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_keeps_real_default() {
    let input = r#"
exports.default = value;
module.exports = exports.default;
"#;
    let expected = "export default value;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_blocks_unsafe_intervening_call() {
    let input = r#"
exports.default = value;
mutate(exports);
module.exports = exports.default;
"#;
    // `mutate(exports)` blocks the mirror, and the call still passes the whole
    // `exports` object after conversion, so the module stays CommonJS.
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn module_exports_default_mirror_blocks_rebinding_exports() {
    let input = r#"
exports.default = value;
exports = other;
module.exports = exports.default;
"#;
    let output = common::render_rule(input, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, input);
}

#[test]
fn module_exports_default_mirror_allows_safe_intervening_aliases() {
    let input = r#"
exports.default = value;
var imported;
imported = dependency;
var alias = imported;
module.exports = exports.default;
"#;
    let expected = r#"
export default value;
let imported;
imported = dependency;
const alias = imported;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_keeps_alias_value() {
    let input = r#"
const makeDefault = () => ({});
const entry = makeDefault;
exports.default = entry;
module.exports = exports.default;
"#;
    let expected = r#"
const makeDefault = () => ({});
export default makeDefault;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_dedup_void_init() {
    // void 0 → undefined after RemoveVoid rule, but the un_esm rule runs and detects void expr
    let input = r#"
exports.foo = void 0;
exports.foo = 1;
"#;
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn global_value_export_declares_a_local_binding() {
    // `export { window as WIN }` would name a binding the module never
    // declares. The CommonJS write copies the value, so a const does too.
    let input = r#"
exports.WIN = window;
exports.EventTarget = EventTarget;
"#;
    let expected = r#"
export const WIN = window;
const _EventTarget = EventTarget;
export { _EventTarget as EventTarget };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn void_of_a_call_is_not_an_export_sentinel() {
    // Only `void <literal>` is a placeholder. `void f()` still calls `f`, and
    // as the last write it is the export's real value.
    let input = r#"
exports.foo = void first();
exports.foo = 1;
exports.bar = void second();
"#;
    let expected = r#"
export let foo = void first();
foo = 1;
export const bar = void second();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn repeated_top_level_writes_keep_every_write() {
    // Each write is a write to the property storage, including the earlier
    // value that code between the writes can observe.
    let input = r#"
exports.foo = sideEffect1();
setup();
exports.foo = sideEffect2();
"#;
    let expected = r#"
export let foo = sideEffect1();
setup();
foo = sideEffect2();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn dropped_default_sentinel_leaves_no_statement() {
    // Babel declares the default export with a `void 0` placeholder before
    // assigning the real value. Evaluating the placeholder has no effect.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.default = void 0;
var _default = function (a) { return a; };
exports.default = _default;
module.exports = exports.default;
"#;
    let expected = r#"
"use strict";
const _default = (a)=>a;
export default _default;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn dropped_sequence_default_sentinel_leaves_no_statement() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: !0 }), exports.default = void 0, exports.default = (a)=>a.b, module.exports = exports.default;
"#;
    let expected = r#"
"use strict";
export default ((a)=>a.b);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn dropped_module_exports_of_hoisted_binding_leaves_no_statement() {
    // Reading a function or `var` binding cannot throw, so the dropped
    // `module.exports = r` needs no leftover read.
    let input = r#"
function r(a) { return a; }
var s = 1;
module.exports = r;
module.exports.default = r;
exports.value = s;
exports.value = 2;
"#;
    let expected = r#"
function r(a) {
    return a;
}
const s = 1;
export default r;
export const value = 2;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn dropped_export_keeps_read_that_can_throw() {
    // A lexical binding read can throw before initialization, and an
    // unresolved read can throw a ReferenceError. Keep both reads.
    let input = r#"
module.exports = early;
let early = 1;
module.exports.default = early;
exports.other = missing;
exports.other = 2;
"#;
    let expected = r#"
early;
let early = 1;
export default early;
missing;
export const other = 2;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn non_top_level_require_unchanged() {
    // VarDeclToLetConst converts var to const since bar is never reassigned.
    let input = r#"
function fn() {
  var bar = require('bar');
}
"#;
    let expected = r#"
function fn() {
  const bar = require('bar');
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_with_prop() {
    let input = "module.exports.foo = 1;";
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_named_diff_ident() {
    // UnEsm produces: function bar() {} + export { bar as foo }
    // UnExportRename then renames `bar` → `foo` and promotes to `export function foo() {}`
    let input = r#"
function bar() {}
exports.foo = bar;
"#;
    let expected = r#"export function foo() {}"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_default_prop_module_exports() {
    let input = "module.exports.default = 42;";
    let expected = "export default 42;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn webpack_export_getter_iife_becomes_named_exports() {
    let input = r#"
((exports_1, B)=>{
  for (const G in B) {
    Object.defineProperty(exports_1, G, {
      enumerable: true,
      get: B[G]
    });
  }
})(exports, {
  Foo() { return A; },
  Bar() { return B; }
});
const A = 1;
const B = 2;
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

const DEFAULT_COMPAT_POSTAMBLE: &str = r#"
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#;

#[test]
fn named_only_commonjs_surface_drops_dead_default_compat_postamble() {
    let input = format!(
        r#"
const answer = 42;
exports.answer = answer;
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );
    let output = apply(&input);
    assert_eq_normalized(&output, "export const answer = 42;");
    assert!(
        validate_output_modules(&[("entry.js".into(), output)]).is_empty(),
        "the recovered named-only module should not retain CommonJS residuals"
    );
}

#[test]
fn logical_expression_default_compat_postamble_is_proven_after_normalization() {
    let input = r#"
const answer = 42;
exports.answer = answer;
("function" == typeof exports.default || "object" == typeof exports.default && null !== exports.default) && void 0 === exports.default.__esModule && (Object.defineProperty(exports.default, "__esModule", {
  value: true
}), Object.assign(exports.default, exports), module.exports = exports.default);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, "export const answer = 42;");
}

/// The postamble reads `exports` until the getter is converted; only then can
/// it be proven dead, so the module must not be rolled back before that.
#[test]
fn getter_exports_beside_a_dead_default_compat_postamble_convert() {
    let input = format!(
        r#"
Object.defineProperty(exports, "getLocale", {{ enumerable: true, get: function () {{ return getLocale; }} }});
function getLocale(value) {{ return false; }}
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );
    let output = apply(&input);
    assert!(
        output.contains("export { getLocale }") || output.contains("export function getLocale"),
        "{output}"
    );
    assert!(!output.contains("exports"), "{output}");
}

#[test]
fn recovered_default_keeps_default_compat_postamble() {
    let input = format!(
        r#"
const answer = 42;
exports.default = answer;
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );
    let output = apply(&input);
    // The postamble is not dead when a default exists, and it reads the whole
    // `exports` object, so the module stays CommonJS.
    assert!(output.contains("exports.default = answer"), "{output}");
    assert!(
        output.contains("Object.assign(exports.default, exports)")
            && output.contains("module.exports = exports.default"),
        "default-object compatibility is not dead when a default exists:\n{output}"
    );
}

#[test]
fn webpack_default_only_getter_rewrites_default_compat_postamble() {
    // A lowered `require.d` getter takes the same default-only compatibility
    // rewrite as an `Object.defineProperty` getter.
    let input = format!(
        r#"
require.d(exports, {{
  default() {{ return answer; }}
}});
const answer = 42;
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );
    assert_eq_normalized(
        &apply(&input),
        r#"
const answer = 42;
export { answer as default };
if ((typeof answer === "function" || typeof answer === "object" && answer !== null) && answer.__esModule === undefined) {
    Object.defineProperty(answer, "__esModule", {
        value: true
    });
    answer.default = answer;
}
"#,
    );
}

#[test]
fn recovered_default_only_getter_rewrites_default_compat_postamble() {
    let input = r#"
Object.defineProperty(exports, "__esModule", {
  value: true
});
Object.defineProperty(exports, "default", {
  enumerable: true,
  get: function() {
    return entry;
  }
});
function entry() {}
("function" == typeof exports.default || "object" == typeof exports.default && null !== exports.default) && void 0 === exports.default.__esModule && (Object.defineProperty(exports.default, "__esModule", {
  value: true
}), Object.assign(exports.default, exports), module.exports = exports.default);
"#;

    let output = apply(input);
    assert_eq_normalized(
        &output,
        r#"
function entry() {}
export { entry as default };
if ((typeof entry === "function" || typeof entry === "object" && entry !== null) && entry.__esModule === undefined) {
    Object.defineProperty(entry, "__esModule", {
        value: true
    });
    entry.default = entry;
}
"#,
    );
    assert!(
        validate_output_modules(&[("entry.js".into(), output)]).is_empty(),
        "the rewritten default-only adapter should leave no CommonJS residual"
    );
}

#[test]
fn shadowed_exports_and_module_do_not_block_default_only_postamble_recovery() {
    let input = format!(
        r#"
Object.defineProperty(exports, "__esModule", {{
  value: true
}});
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{
    return entry;
  }}
}});
function entry(exports, module) {{
  return exports ?? module;
}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );

    let output = apply(&input);
    assert!(!output.contains("Object.assign(exports.default, exports)"));
    assert!(!output.contains("module.exports = exports.default"));
    assert!(output.contains("export { entry as default }"));
    assert!(output.contains("entry.default = entry"));
}

#[test]
fn default_only_type_helper_is_preserved_on_the_recovered_binding() {
    let input = r#"
const typeOf = require("./typeof.js");
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() {
    return entry;
  }
});
function entry() {}
if ((typeof exports.default === "function" || typeOf(exports.default) === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#;

    let output = apply(input);
    assert!(output.contains("typeOf(entry) === \"object\""), "{output}");
    assert!(output.contains("entry.default = entry"), "{output}");
    assert!(!output.contains("exports.default"), "{output}");
    assert!(!output.contains("module.exports"), "{output}");
}

#[test]
fn non_exact_default_surfaces_keep_default_compat_postamble() {
    let cases = [
        (
            "named export getter",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
Object.defineProperty(exports, "answer", {{
  enumerable: true,
  get() {{ return answer; }}
}});
function entry() {{}}
const answer = 42;
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "exports alias",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
const publicApi = exports;
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "other module use",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
inspect(module);
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "direct eval",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{ eval(source); }}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "default assignment instead of generated getter",
            format!("function entry() {{}}\nexports.default = entry;\n{DEFAULT_COMPAT_POSTAMBLE}"),
        ),
        (
            "authored ESM default",
            format!(
                "function entry() {{}}\nexport {{ entry as default }};\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
        (
            "mixed authored ESM surface",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{}}
export const answer = 42;
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "getter re-export",
            format!(
                r#"
const dependency = require("./dependency.js");
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return dependency.default; }}
}});
                {DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "shadowed Object helper",
            format!(
                r#"
const Object = customObject;
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "duplicate default getter",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return replacement; }}
}});
function entry() {{}}
function replacement() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "spread default getter argument",
            format!(
                r#"
Object.defineProperty(...exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "postamble is not final",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
observe(entry);
"#
            ),
        ),
        (
            "postamble has an alternate branch",
            r#"
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return entry; }
});
function entry() {}
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
} else {
  observe(exports.default);
}
"#
            .to_string(),
        ),
        (
            "effectful compatibility descriptor",
            r#"
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return entry; }
});
function entry() {}
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true,
    configurable: touch(exports)
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#
            .to_string(),
        ),
        (
            "multi-argument type helper",
            r#"
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return entry; }
});
function entry() {}
if ((typeof exports.default === "function" || typeOf(exports.default, exports) === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#
            .to_string(),
        ),
        (
            "CommonJS type helper receiver",
            r#"
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return entry; }
});
function entry() {}
if ((typeof exports.default === "function" || exports.typeOf(exports.default) === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#
            .to_string(),
        ),
    ];

    for (name, input) in cases {
        let output = apply(&input);
        assert!(
            output.contains("Object.assign(exports.default, exports)")
                && output.contains("module.exports = exports.default"),
            "{name} must fail closed:\n{output}"
        );
    }
}

#[test]
fn dynamic_commonjs_surfaces_keep_default_compat_postamble() {
    let cases = [
        (
            "computed write",
            format!("const name = chooseName();\nexports[name] = 1;\n{DEFAULT_COMPAT_POSTAMBLE}"),
        ),
        (
            "exports alias",
            format!("const publicApi = exports;\npublicApi.answer = 42;\n{DEFAULT_COMPAT_POSTAMBLE}"),
        ),
        (
            "hidden default descriptor",
            format!(
                "Object.defineProperty(exports, \"default\", {{ value: 42 }});\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
        (
            "dynamic getter helper",
            format!(
                "const name = chooseName();\nrequire.d(exports, name, () => 42);\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
        (
            "direct eval",
            format!("exports.answer = 42;\neval(source);\n{DEFAULT_COMPAT_POSTAMBLE}"),
        ),
        (
            "hoisted default writer",
            format!(
                "exports.answer = 42;\n{DEFAULT_COMPAT_POSTAMBLE}\nfunction installDefault() {{ exports.default = 42; }}"
            ),
        ),
        (
            "prototype reassignment",
            format!(
                "exports.__proto__ = {{ default: {{}} }};\nexports.answer = 42;\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
        (
            "legacy default getter installer",
            format!(
                "exports.answer = 42;\nexports.__defineGetter__(\"default\", () => ({{ answer: exports.answer }}));\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
    ];

    for (name, input) in cases {
        let output = apply(&input);
        assert!(
            output.contains("Object.assign(exports.default, exports)")
                && output.contains("module.exports = exports.default"),
            "{name} must fail closed:\n{output}"
        );
    }
}

#[test]
fn prototype_mutating_exports_write_stays_commonjs_residual() {
    let output = apply("exports.__proto__ = { legacy: true };\nexports.answer = 42;\n");
    assert!(
        !output.contains("export const __proto__"),
        "a prototype write is not a named export:\n{output}"
    );
    assert!(
        output.contains("exports.__proto__"),
        "the prototype write must stay visible as a residual:\n{output}"
    );
    assert!(
        output.contains("export const answer = 42"),
        "ordinary named exports still convert around the residual:\n{output}"
    );
}

#[test]
fn export_getter_map_with_prototype_mutating_name_stays_residual() {
    let input = r#"
require.d(exports, {
  __proto__() { return legacy; },
  real() { return value; }
});
const value = 1;
"#;
    let output = apply(input);
    assert!(
        output.contains("require.d(exports"),
        "converting would synthesize a prototype write for the __proto__ entry:\n{output}"
    );
    assert!(
        !output.contains("export "),
        "no partial conversion of the remaining map entries:\n{output}"
    );
}

#[test]
fn single_export_getter_with_prototype_mutating_name_stays_residual() {
    let output = apply("require.d(exports, \"__proto__\", () => 42);\n");
    assert!(
        output.contains("require.d(exports"),
        "the original own accessor definition must stay as a residual:\n{output}"
    );
}

#[test]
fn webpack_export_getter_iife_recovers_live_default_after_declaration() {
    let input = r#"
((target, getters) => {
  for (const key in getters) {
    Object.defineProperty(target, key, {
      enumerable: true,
      get: getters[key]
    });
  }
})(exports, {
  dim() { return dim; },
  default() { return logger; }
});
function dim(value) {
  return value;
}
const logger = {
  warn(value) { console.warn(value); }
};
"#;
    let output = apply(input);
    assert!(
        !output.contains("Object.defineProperty") && !output.contains("getters"),
        "the recognized getter-map helper should be removed:\n{output}"
    );
    assert!(
        output.contains("export { dim };") || output.contains("export function dim"),
        "the named getter should remain a live named export:\n{output}"
    );
    assert!(
        output.contains("export { logger as default };"),
        "the default getter should remain a live default export:\n{output}"
    );
    let declaration = output
        .find("const logger")
        .expect("logger declaration should remain");
    let export = output
        .find("export { logger as default };")
        .expect("default export should be recovered");
    assert!(
        declaration < export,
        "the default export must be deferred past its declaration:\n{output}"
    );
}

const GETTER_LOOP_IIFE: &str = r#"
((target, getters) => {
  for (const key in getters) {
    Object.defineProperty(target, key, {
      enumerable: true,
      get: getters[key]
    });
  }
})"#;

#[test]
fn webpack_getter_loop_exports_of_written_bindings_stay_live() {
    // The getters read the binding on every access, so a binding written
    // after the getter loop must not become a snapshot export.
    for (getters, rest, expected) in [
        (
            "{ Kind() { return G; } }",
            "var G; G = { A: \"a\" };",
            "export { G as Kind }; var G; G = { A: \"a\" };",
        ),
        (
            "{ Foo() { return A; } }",
            "let A = 1; function bump() { A = 2; }",
            "export { A as Foo }; let A = 1; function bump() { A = 2; }",
        ),
    ] {
        let source = format!("{GETTER_LOOP_IIFE}(exports, {getters}); {rest}");
        let output = common::render_rule(&source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, expected);
    }
}

#[test]
fn webpack_getter_loop_with_default_drops_the_default_compat_postamble() {
    let source = format!(
        "{GETTER_LOOP_IIFE}(exports, {{ Foo() {{ return A; }}, default() {{ return D; }} }}); let A = 1; function D() {{}}{DEFAULT_COMPAT_POSTAMBLE}"
    );
    let output = common::render_rule(&source, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(
        &output,
        "export { A as Foo }; let A = 1; function D() {} export { D as default };",
    );
}

#[test]
fn webpack_getter_default_deferred_to_end() {
    // Webpack5 export getters place the getter map at the top of the module,
    // before declarations.  Named exports are fine (live bindings), but
    // `default` exports evaluate eagerly.  The default entry must be deferred
    // to the end of the module body to avoid TDZ violations.
    let input = r#"
require.d(exports, {
  default() { return o; },
  VERSION() { return VERSION; }
});
const r = { apiBase: "https://example.com" };
const o = r;
const VERSION = "2.1.0";
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn direct_webpack_export_getters_become_named_exports() {
    let input = r#"
require.d(exports, "APP_NAME", ()=>n);
require.d(exports, "readSetting", ()=>i);
const n = "Revenue Console";
function i(t, e = null) {
  return e;
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn webpack_array_form_values_and_getters_become_exports() {
    // webpack 5.108+ defines `const` exports at the end of the module with an
    // array: a `0` slot is followed by the value of a data property, any
    // other slot is a getter.
    let input = r#"
require.d(exports, { f: () => f });
const limit = 1;
const config = { a: 1 };
function f() { return limit; }
let count = 0;
require.d(exports, ["cfg", 0, config, "count", () => count, "limit", 0, limit]);
"#;
    let expected = r#"
const limit = 1;
const config = {
    a: 1
};
function f() {
    return limit;
}
export { f };
export let count = 0;
export { config as cfg };
export { limit };
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn rspack_value_definitions_become_exports() {
    // rspack's `require.d(exports, getters, values)` defines each entry of
    // the third object as a data property holding its value.
    let input = r#"
const table = { a: 1 };
function f() { return table; }
require.d(exports, { f: () => f }, { table });
"#;
    let expected = r#"
const table = {
    a: 1
};
function f() {
    return table;
}
export { f };
export { table };
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn webpack_getter_returning_a_require_member_becomes_a_re_export() {
    let input = r#"
require.d(exports, ["take", () => effects.take]);
var effects = require("./effects.js");
"#;
    assert_eq_normalized(&apply(input), r#"export { take } from "./effects.js";"#);
}

#[test]
fn webpack_getter_of_unresolved_module_id_member_becomes_snapshot_after_require() {
    // A module id the unpacker could not resolve has no source to re-export
    // from; the getter's value once the require has run is what remains.
    let input = r#"
require.d(exports, { take: () => effects.take, local: () => local });
var effects = require(11111);
var other = require("./other.js");
var local = other.make();
"#;
    let expected = r#"
import other from "./other.js";
const effects = require(11111);
export const take = effects.take;
export const local = other.make();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn webpack_getter_of_unresolved_module_id_member_stays_behind_other_code() {
    let input = r#"
require.d(exports, { take: () => effects.take, local: () => local });
var local = 1;
var effects = require(11111);
"#;
    let output = apply(input);
    assert!(
        output.contains("Object.defineProperty(exports, \"take\""),
        "{output}"
    );
    assert!(!output.contains("export const take"), "{output}");
}

#[test]
fn module_that_stays_commonjs_keeps_webpack_definitions_as_written() {
    // The lowered getter definitions are only an intermediate form for
    // conversion; an aliased `exports` keeps the module CommonJS.
    let input = r#"
require.d(exports, { a: () => a });
var alias = exports;
alias.extra = 1;
const a = 1;
"#;
    let output = apply(input);
    assert!(output.contains("require.d(exports, {"), "{output}");
    assert!(!output.contains("defineProperty"), "{output}");
}

#[test]
fn repeated_webpack_definition_keys_are_not_lowered() {
    // The runtime skips a key `exports` already owns, while a second
    // `Object.defineProperty` of the same key would throw.
    let input = r#"
require.d(exports, { a: () => a });
require.d(exports, ["a", 0, b]);
const a = 1;
const b = 2;
"#;
    let output = apply(input);
    assert!(!output.contains("defineProperty"), "{output}");
    assert_eq!(output.matches("require.d(exports").count(), 2, "{output}");
}

#[test]
fn webpack_export_getter_to_mutable_binding_stays_live() {
    let input = r#"
let value = 1;
require.d(exports, "value", () => value);
value = 2;
"#;
    let expected = r#"
export let value = 1;
value = 2;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn direct_webpack_export_getter_member_return_does_not_leak_helper() {
    let input = r#"
const effects = require("./effects.js");
require.d(exports, "take", ()=>effects.take);
"#;
    let output = apply(input);
    assert!(
        !output.contains("require.d"),
        "webpack export getter helper should not survive:\n{output}"
    );
    insta::assert_snapshot!(output);
}

#[test]
fn direct_webpack_export_getter_map_becomes_named_exports() {
    let input = r#"
require.d(exports, {
  APP_NAME() { return n; },
  readSetting() { return i; }
});
const n = "Revenue Console";
function i(t, e = null) {
  return e;
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn unused_iife_with_webpack_export_getters_becomes_module_exports() {
    let input = r#"
"use strict";
((t)=>{
  require.d(exports, "VERSION", ()=>o);
  require.d(exports, "getConfig", ()=>i);
  require.d(exports, "mergeConfig", ()=>u);
  const r = {
    apiBase: "https://example.com",
    timeout: 5000
  };
  exports.default = r;
  const o = "2.1.0";
  function i(t) {
    return r[t];
  }
  function u(t) {
    return { ...r, ...t };
  }
})(require("./module-11.js"));
"#;
    let output = apply(input);
    assert!(
        !output.contains("require.d"),
        "webpack export getter helper should not survive:\n{output}"
    );
    assert!(
        output.contains("\"use strict\""),
        "the leading strict directive must survive IIFE exposure:\n{output}"
    );
    insta::assert_snapshot!(output);
}

#[test]
fn iife_with_used_param_keeps_webpack_export_getter_wrapped() {
    let input = r#"
((t)=>{
  require.d(exports, "value", ()=>t.value);
})(require("./dep.js"));
"#;
    let output = apply(input);
    assert!(
        output.contains("require.d"),
        "webpack export getter should stay wrapped when the IIFE param is used:\n{output}"
    );
    insta::assert_snapshot!(output);
}

#[test]
fn webpack_export_getter_iife_keeps_non_compat_if_block() {
    let input = r#"
((exports_1, B)=>{
  for (const G in B) {
    Object.defineProperty(exports_1, G, {
      enumerable: true,
      get: B[G]
    });
  }
})(exports, {
  Foo() { return A; }
});
const A = 1;
if (flag) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn void_only_export_is_an_uninitialized_export() {
    // TypeScript emits only the sentinel for `export let foo;`. The property
    // exists, so an importer of `foo` must still link.
    let input = "exports.foo = void 0;";
    let expected = "export let foo;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn self_ref_pattern_removed() {
    let input = "module.exports.default = module.exports;";
    let expected = "";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn coupled_lazy_default_helper_uses_live_binding_and_preserves_self_mirrors() {
    let input = r#"
function helper(value) {
  module.exports = helper = (next) => typeof next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    let expected = r#"
function helper(value) {
  helper = next => typeof next;
  helper.default = helper;
  return helper(value);
}
export { helper as default };
helper.default = helper;
"#;
    assert_eq_normalized(&output, expected);
    assert!(
        validate_output_modules(&[("entry.js".into(), output)]).is_empty(),
        "the recovered live default should be a valid ESM module"
    );
}

#[test]
fn coupled_lazy_default_helper_ignores_shadowed_module_bindings() {
    let input = r#"
function helper(value) {
  {
    const module = { exports: "local" };
    consume(module.exports);
  }
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(module.exports)"),
        "a lexically shadowed module binding must remain untouched:\n{output}"
    );
    assert_eq!(
        output.matches("module.exports").count(),
        1,
        "only the shadowed local read should remain:\n{output}"
    );
    assert!(output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_handles_mutually_exclusive_replacements() {
    let input = r#"
function helper(value) {
  if (supportsFastPath) {
    module.exports = helper = fastPath;
  } else {
    module.exports = helper = slowPath;
  }
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        !output.contains("module.exports"),
        "every whole-value replacement is coupled to the same helper binding:\n{output}"
    );
    assert!(
        output.contains("helper = fastPath") && output.contains("helper = slowPath"),
        "both runtime branches must keep their original binding updates:\n{output}"
    );
    assert!(output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_handles_called_sequence_value() {
    let input = r#"
function helper() {
  return (module.exports = helper = () => true,
    module.exports.__esModule = true,
    module.exports.default = module.exports)();
}
module.exports = helper;
module.exports.__esModule = true;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        !output.contains("module.exports"),
        "the called sequence should use the proven coupled binding:\n{output}"
    );
    assert!(
        output.contains("helper.__esModule = true"),
        "a nested marker that is part of the runtime value must stay observable:\n{output}"
    );
    assert!(output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_uncoupled_module_replacement() {
    let input = r#"
function helper(value) {
  module.exports = chooseOtherValue();
  module.exports.default = module.exports;
  return value;
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports = chooseOtherValue()")
            && output.contains("module.exports.default = module.exports"),
        "an uncoupled CommonJS replacement must remain visible:\n{output}"
    );
    assert!(
        !output.contains("export { helper as default }"),
        "the ordinary snapshot default must not be upgraded without coupling proof:\n{output}"
    );
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_independent_binding_write() {
    let input = r#"
function helper(value) {
  helper = chooseOtherValue();
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports"),
        "the helper and CommonJS value can diverge before the coupled write:\n{output}"
    );
    assert!(!output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_direct_eval() {
    let input = r#"
function helper(value) {
  eval(source);
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports"),
        "direct eval can observe or mutate both candidate identities:\n{output}"
    );
    assert!(!output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_receiver_sensitive_call() {
    let input = r#"
function helper(value) {
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return module.exports(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports(value)"),
        "a direct member call supplies the CommonJS module as `this`:\n{output}"
    );
    assert!(!output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_exports_alias_use() {
    let input = r#"
function helper(value) {
  module.exports = helper = (next) => next;
  exports.alias = helper;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports"),
        "exports still names the initial object after module.exports is replaced:\n{output}"
    );
    assert!(!output.contains("export { helper as default }"), "{output}");
}

#[test]
fn existing_import_absorbed() {
    let input = r#"
import { a } from 'foo';
var { b } = require('foo');
"#;
    let expected = r#"import { a, b } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn compound_assign_not_transformed() {
    // module.exports += 1 should NOT be transformed
    let input = "module.exports += 1;";
    let expected = "module.exports += 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn bracket_notation_module_exports_transformed() {
    // module["exports"] is normalized to module.exports by UnBracketNotation,
    // then converted to ESM by UnEsm
    let input = r#"module["exports"] = 1;"#;
    let expected = "export default 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_name_takes_priority_over_conflicting_local() {
    // When exports.a = expr and `a` is already a local binding,
    // the local should be renamed so the export keeps the clean name.
    let input = r#"
var a = 0;
exports.a = function(x) { return a + x; };
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn export_conflict_rename_avoids_nested_shadow_capture() {
    let input = r#"
var a = 0;
function f(_a) { return a + _a; }
exports.a = function(x) { return a + f(x); };
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn export_conflict_rename_preserves_object_pattern_key() {
    let input = r#"
var obj = { a: 1 };
var { a } = obj;
exports.a = function(x) { return a + x; };
"#;
    let output = render_pipeline_until(input, "UnEsm");
    // Destructuring must produce `{ a: _a }`, not `{ _a }` — the property key stays `a`.
    insta::assert_snapshot!(output);
}

#[test]
fn no_rename_when_export_name_is_free() {
    // No conflict — export name is not used by any local binding
    let input = r#"
var b = 0;
exports.a = function(x) { return b + x; };
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn reserved_named_export_uses_safe_local_binding() {
    let input = r#"
exports.eval = function(source) {
    return eval(source);
};
exports.in = 1;
"#;
    let expected = r#"
var _eval = function(source) {
    return eval(source);
};
export { _eval as eval };
var _in = 1;
export { _in as in };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_export_does_not_capture_existing_global_reference() {
    let input = r#"
var marker = typeof runtime !== "undefined" && runtime.pid ? runtime.pid : "";
module.exports = function() {
    return marker;
};
module.exports.runtime = function() {
    return marker;
};
"#;
    let expected = r#"
const marker = typeof runtime !== "undefined" && runtime.pid ? runtime.pid : "";
export default function() { return marker; };
const _runtime = function() {
    return marker;
};
export { _runtime as runtime };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn compound_exports_assignment_in_var_decl() {
    // var s = exports.history = expr → split into var s = expr + export { s as history }
    let input = r#"
var s = exports.history = createBrowserHistory();
use(s);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn compound_exports_same_name_merges_to_export_decl() {
    // var SessionContext = exports.SessionContext = expr
    // → export var SessionContext = expr (merge preserves original decl kind)
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.SessionContext = void 0;
var SessionContext = exports.SessionContext = React.createContext(undefined);
use(SessionContext);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn compound_exports_declarator_exports_one_binding() {
    for (source, expected) in [
        // The local is written later: the export is a snapshot of the first
        // value, next to the local declaration.
        (
            "var s = exports.history = create(); s = 2; use(s);",
            "var s = create(); export const history = s; s = 2; use(s);",
        ),
        // The property is written later: the export is the property's own
        // binding, declared by the first write.
        (
            "var s = exports.history = create(); exports.history = 3;",
            "export var history = create(); history = 3;",
        ),
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, expected);
    }
}

// ============================================================
// Require hoisting from complex expressions
// ============================================================

#[test]
fn hoist_require_from_seq_expr_in_export_default() {
    let input = r#"
let i;
export default (i = require("./a.js"), require("./b.js"), i.foo);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn hoist_require_call_invocation() {
    let input = r#"
export default require("./factory.js")();
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn inline_conditional_interop_to_import() {
    let input = r#"
let i;
const a = (i = require("./react.js")) && i.__esModule ? i : { default: i };
console.log(a);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn inline_conditional_interop_default_only_to_default_import() {
    let input = r#"
let n;
const r = (n = require("./base.js")) && n.__esModule ? n : { default: n };
function build() {
  return factory(r.default);
}
"#;
    let expected = r#"
import r from "./base.js";
let n;
function build() {
  return factory(r);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);

    let expected_final = r#"
import r from "./base.js";
function build() {
  return factory(r);
}
"#;
    assert_eq_normalized(&render_pipeline(input), expected_final);
}

#[test]
fn inline_conditional_interop_default_recovery_is_binding_aware() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
function read(r) {
  return r.default;
}
consume(r.default, read(other()));
"#;
    let expected = r#"
import r from "./dep.js";
let n;
function read(r) {
  return r.default;
}
consume(r, read(other()));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inline_conditional_interop_default_recovery_handles_optional_member_read() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
consume(r?.default);
"#;
    let output = apply(input);
    assert!(
        !output.contains(".default") || !output.contains("import r from"),
        "default-only recovery must rewrite every accepted access: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_rejects_mixed_wrapper_uses() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
consume(r.default, r);
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(r.default, r)"),
        "a wrapper that escapes must keep its Babel interop semantics: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_rejects_writes() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
r.default = replacement;
"#;
    let output = apply(input);
    assert!(
        output.contains(".default = replacement") && !output.contains("import r from"),
        "a written wrapper property must not use the default-only recovery: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_rejects_dynamic_properties() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
consume(r[key]);
"#;
    let output = apply(input);
    assert!(
        output.contains("[key]") && !output.contains("import r from"),
        "a dynamically accessed wrapper must not use the default-only recovery: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_rejects_used_require_temp() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
consume(r.default, n);
"#;
    let output = apply(input);
    assert!(
        output.contains("n = _n")
            && output.contains("consume(n.default, n)")
            && !output.contains("import r from"),
        "a require temp used outside the helper must keep its assignment: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_preserves_late_let_tdz() {
    let input = r#"
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
let n;
consume(r.default);
"#;
    let expected = r#"
import _n from "./dep.js";
n = _n;
const r = n;
let n;
consume(r.default);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inline_conditional_interop_rejects_mismatched_shape() {
    let input = r#"
let i;
let j;
const a = (i = require("./react.js")) && j.__esModule ? i : { default: j };
"#;
    let output = apply(input);
    assert!(
        output.contains("require(\"./react.js\")") && output.contains("j.__esModule"),
        "mismatched inline conditional should not be hoisted as Babel interop: {output}"
    );
}

#[test]
fn plain_export_default_require_not_hoisted() {
    // export default require("...") should NOT be hoisted — it's a valid re-export
    // that namespace_decomposition can see through.
    let input = r#"
export default require("./module.js");
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_compound_module_write() {
    // `*=` reads module.exports before writing; deleting the read-modify-
    // write is not a coupled replacement.
    let input = r#"
function helper(value) {
  module.exports *= helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports *="),
        "a compound CommonJS write must remain visible:\n{output}"
    );
    assert!(
        !output.contains("export { helper as default }"),
        "compound writes must fail the coupling proof:\n{output}"
    );
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_logical_module_write() {
    // `||=` only evaluates its right side when module.exports is falsy;
    // the original never reassigns the helper here.
    let input = r#"
function helper(value) {
  module.exports ||= helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports ||="),
        "a conditional CommonJS write must remain visible:\n{output}"
    );
    assert!(
        !output.contains("export { helper as default }"),
        "logical assignment must fail the coupling proof:\n{output}"
    );
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_optional_chained_module_use() {
    // The rewriter substitutes only plain `module.exports` members; an
    // optional-chained access would survive as an orphaned free `module`.
    let input = r#"
function helper(value) {
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return module?.exports(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports = helper"),
        "the coupled write must stay when an optional-chained use exists:\n{output}"
    );
    assert!(
        !output.contains("export { helper as default }"),
        "optional-chained module access must fail the coupling proof:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// Top-level Call args: require("mod").Name → named import
//
// Producer: a CJS compiler emits a static `require(mod).Name` as a direct
// argument of an immediately-evaluated top-level call (typically an IIFE).
// UnEsm already converts `var x = require("mod").Name` via NamedProp; this
// shape never reached that classifier. `.default` args use a parallel pass.
// ---------------------------------------------------------------------------

fn apply_unesm(input: &str) -> String {
    render_pipeline_until(input, "UnEsm")
}

#[test]
fn toplevel_iife_require_named_member_arg_to_named_import() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
    assert!(
        !output.contains("require("),
        "the hoisted named member must become an import:\n{output}"
    );
}

#[test]
fn toplevel_iife_require_named_member_arg_survives_later_rules() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply(input);
    assert!(
        output.contains("import { UIBase } from \"./UIBase.js\"") && !output.contains("require("),
        "later rules must keep the named import:\n{output}"
    );
}

#[test]
fn toplevel_var_init_iife_require_named_member_arg_to_named_import() {
    let input = r#"
var Child = (function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
var Child = function (base) {
  use(base);
}(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_iife_computed_ident_require_named_member_arg_to_named_import() {
    // UnBracketNotation may already fold this to `.UIBase`; UnEsm must still
    // accept `is_ident_prop` computed strings if the member survives.
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js")["UIBase"]);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_arg_reuses_existing_named_prop() {
    let input = r#"
var UIBase = require("./UIBase.js").UIBase;
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_arg_reuses_existing_named_prop_through_full_pipeline() {
    // Replacing the argument with make_ident() would drop the resolved ctxt of
    // `var UIBase`, so DeadImports would treat the named import as unused.
    let input = r#"
var UIBase = require("./UIBase.js").UIBase;
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_iife_require_named_member_arg_keeps_import_through_full_pipeline() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_does_not_reuse_later_named_prop() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
var UIBase = require("./UIBase.js").UIBase;
var keep = require("./keep.js");
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_does_not_reuse_mutated_named_prop() {
    let input = r#"
var UIBase = require("./UIBase.js").UIBase;
UIBase = other;
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
var keep = require("./keep.js");
"#;
    let output = apply_unesm(input);
    assert!(
        output.contains("require(\"./UIBase.js\").UIBase")
            && output.contains("import keep from \"./keep.js\""),
        "a mutated NamedProp local must not be reused as the call argument:\n{output}"
    );
}

#[test]
fn toplevel_iife_require_default_member_arg_to_default_import() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
    assert!(
        !output.contains("require("),
        "the hoisted default member must become an import:\n{output}"
    );
}

#[test]
fn toplevel_require_default_member_uses_readable_fallback_for_numeric_module_name() {
    let input = r#"
(function (base) {
  use(base);
})(require("./module-42.js").default);
consume(require("./module-43.js").default);
consume(require("./module-44.js").default);
"#;
    let expected = r#"
import defaultExport from "./module-42.js";
import defaultExport_1 from "./module-43.js";
import defaultExport_2 from "./module-44.js";
(function (base) {
  use(base);
})(defaultExport);
consume(defaultExport_1);
consume(defaultExport_2);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_avoids_readable_fallback_collision() {
    let input = r#"
let defaultExport = existing;
consume(require("./module-42.js").default);
"#;
    let expected = r#"
import defaultExport_1 from "./module-42.js";
let defaultExport = existing;
consume(defaultExport_1);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_ternary_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
f(cond ? require("./UIBase.js").UIBase : other);
"#;
    let expected = r#"
import keep from "./keep.js";
f(cond ? require("./UIBase.js").UIBase : other);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_inside_then_callback_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
then(function () {
  return require("./UIBase.js").UIBase;
});
"#;
    let expected = r#"
import keep from "./keep.js";
then(function () {
  return require("./UIBase.js").UIBase;
});
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_inside_function_body_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
function wrap() {
  (function (base) {
    use(base);
  })(require("./UIBase.js").UIBase);
}
"#;
    let expected = r#"
import keep from "./keep.js";
function wrap() {
  (function (base) {
    use(base);
  })(require("./UIBase.js").UIBase);
}
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_dynamic_require_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require(dyn).UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require(dyn).UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_spread_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
f(...require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
f(...require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_computed_dynamic_key_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js")[key]);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./UIBase.js")[key]);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_local_require_binding_is_left_alone() {
    let input = r#"
function require(x) {
  return x;
}
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn toplevel_require_named_member_comma_expr_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})((0, require("./UIBase.js").UIBase));
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})((0, require("./UIBase.js").UIBase));
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_existing_import_local() {
    let input = r#"
import { UIBase } from "./other.js";
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./other.js";
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_existing_let_binding() {
    let input = r#"
let UIBase = 0;
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
let UIBase = 0;
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_unresolved_reference() {
    let input = r#"
observe(UIBase);
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
observe(UIBase);
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_unresolved_assignment_target() {
    let input = r#"
UIBase = globalValue;
var keep = require("./keep.js");
consume(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
UIBase = globalValue;
consume(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_unresolved_jsx_tag() {
    let input = r#"
render(<UIBase />);
var keep = require("./keep.js");
consume(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
render(<UIBase />);
consume(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_two_sources_one_local() {
    let input = r#"
var keep = require("./keep.js");
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(require("./B.js").A);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(require("./B.js").A);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_eval_of_name() {
    let input = r#"
eval("UIBase");
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
eval("UIBase");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_dynamic_eval() {
    let input = r#"
eval(source);
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
eval(source);
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_reserved_prop() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./X.js").class);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./X.js").class);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_illegal_ident_prop() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./X.js")["foo-bar"]);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./X.js")["foo-bar"]);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_invalid_unicode_ident_prop() {
    let input = r#"
var keep = require("./keep.js");
f(require("./X.js")["a²"]);
"#;
    let expected = r#"
import keep from "./keep.js";
f(require("./X.js")["a²"]);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_early_self_read_keeps_commonjs_boundary() {
    let input = r#"
consume(require("./module-1.js").value);
exports.value = 1;
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, input);
    assert!(
        !output.contains("import ") && !output.contains("export "),
        "a direct named self-read must not cross only part of its CommonJS boundary:\n{output}"
    );
}

#[test]
fn toplevel_require_named_member_fails_closed_on_provider_member_write() {
    let input = r#"
consume(require("./dep.js").UIBase);
require("./dep.js").UIBase = replacement;
consume(require("./dep.js").UIBase);
"#;
    let output = apply_unesm(input);

    assert_eq_normalized(&output, input);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_other_provider_member_mutations() {
    let mutations = [
        r#"require("./dep.js").UIBase += replacement;"#,
        r#"require("./dep.js").UIBase++;"#,
        r#"delete require("./dep.js").UIBase;"#,
        r#"for (require("./dep.js").UIBase in values) {}"#,
        r#"for (require("./dep.js").UIBase of values) {}"#,
    ];

    for mutation in mutations {
        let input = format!(
            r#"
consume(require("./dep.js").UIBase);
{mutation}
consume(require("./dep.js").UIBase);
"#
        );
        let output = apply_unesm(&input);

        assert!(
            !output.contains("import { UIBase }")
                && output.matches("require(\"./dep.js\").UIBase").count() >= 2,
            "provider mutation must keep fresh member reads:\n{output}"
        );
    }
}

#[test]
fn toplevel_require_named_member_does_not_reuse_local_across_provider_member_write() {
    let input = r#"
var UIBase = require("./dep.js").UIBase;
require("./dep.js").UIBase = replacement;
consume(require("./dep.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./dep.js";
require("./dep.js").UIBase = replacement;
consume(require("./dep.js").UIBase);
"#;
    let output = apply_unesm(input);

    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_sibling_named_import_does_not_block_recovery() {
    let input = r#"
import { keep } from "./keep.js";
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import { keep } from "./keep.js";
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_var_init_iife_require_default_member_arg_to_default_import() {
    let input = r#"
var Child = (function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
var Child = function (base) {
  use(base);
}(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_iife_computed_require_default_member_arg_to_default_import() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js")["default"]);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_arg_reuses_existing_default_prop() {
    let input = r#"
var UIBase = require("./UIBase.js").default;
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_arg_reuses_existing_default_import() {
    let input = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_promotes_type_only_import_for_runtime_value() {
    let input = r#"
import type UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = wakaru_core::decompile(
        input,
        wakaru_core::DecompileOptions {
            filename: "fixture.ts".to_string(),
            ..Default::default()
        },
    )
    .expect("TypeScript input should decompile")
    .code;
    let expected = r#"
import UIBase from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;

    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_arg_reuses_existing_default_import_through_full_pipeline() {
    let input = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_arg_reuses_existing_default_prop_through_full_pipeline() {
    let input = r#"
var UIBase = require("./UIBase.js").default;
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_iife_require_default_member_arg_keeps_import_through_full_pipeline() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_shares_local_across_same_source_args() {
    let input = r#"
(function (first) {
  use(first);
})(require("./UIBase.js").default);
(function (second) {
  use(second);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
(function (first) {
  use(first);
})(UIBase);
(function (second) {
  use(second);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_when_basename_matches_import() {
    let input = r#"
import { UIBase } from "./other.js";
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import { UIBase } from "./other.js";
import keep from "./keep.js";
import defaultExport from "./UIBase.js";
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_when_basename_matches_let() {
    let input = r#"
let UIBase = 0;
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import defaultExport from "./UIBase.js";
let UIBase = 0;
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_when_basename_is_unresolved() {
    let input = r#"
observe(UIBase);
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import defaultExport from "./UIBase.js";
observe(UIBase);
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_for_unresolved_assignment_target() {
    let input = r#"
UIBase = globalValue;
consume(require("./UIBase.js").default);
"#;
    let expected = r#"
import defaultExport from "./UIBase.js";
UIBase = globalValue;
consume(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_for_named_default_declaration() {
    let input = r#"
export default function UIBase() {}
consume(require("./UIBase.js").default);
"#;
    let expected = r#"
import defaultExport from "./UIBase.js";
export default function UIBase() {}
consume(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_ternary_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
f(cond ? require("./UIBase.js").default : other);
"#;
    let expected = r#"
import keep from "./keep.js";
f(cond ? require("./UIBase.js").default : other);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_inside_then_callback_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
then(function () {
  return require("./UIBase.js").default;
});
"#;
    let expected = r#"
import keep from "./keep.js";
then(function () {
  return require("./UIBase.js").default;
});
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_inside_function_body_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
function wrap() {
  (function (base) {
    use(base);
  })(require("./UIBase.js").default);
}
"#;
    let expected = r#"
import keep from "./keep.js";
function wrap() {
  (function (base) {
    use(base);
  })(require("./UIBase.js").default);
}
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_dynamic_require_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require(dyn).default);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require(dyn).default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_spread_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
f(...require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
f(...require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_local_require_binding_is_left_alone() {
    let input = r#"
function require(x) {
  return x;
}
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn toplevel_require_default_member_comma_expr_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})((0, require("./UIBase.js").default));
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})((0, require("./UIBase.js").default));
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_fails_closed_on_unknown_eval() {
    let input = r#"
eval(source);
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
eval(source);
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_when_eval_mentions_basename() {
    let input = r#"
eval("UIBase");
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import defaultExport from "./UIBase.js";
eval("UIBase");
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_fails_closed_on_eval_of_synthetic_name() {
    let input = r#"
import { UIBase } from "./other.js";
eval("defaultExport");
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import { UIBase } from "./other.js";
import keep from "./keep.js";
eval("defaultExport");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_early_self_read_keeps_commonjs_boundary() {
    let input = r#"
consume(require("./module-1.js").default);
exports.default = 1;
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, input);
    assert!(
        !output.contains("import ") && !output.contains("export "),
        "a direct default self-read must not cross only part of its CommonJS boundary:\n{output}"
    );
}

#[test]
fn toplevel_require_default_member_fails_closed_on_provider_member_write() {
    let input = r#"
consume(require("./dep.js").default);
require("./dep.js").default = replacement;
consume(require("./dep.js").default);
"#;
    let output = apply_unesm(input);

    assert_eq_normalized(&output, input);
}

#[test]
fn toplevel_require_default_member_fails_closed_on_other_provider_member_mutations() {
    let mutations = [
        r#"require("./dep.js").default += replacement;"#,
        r#"require("./dep.js").default++;"#,
        r#"delete require("./dep.js").default;"#,
        r#"for (require("./dep.js").default in values) {}"#,
        r#"for (require("./dep.js").default of values) {}"#,
    ];

    for mutation in mutations {
        let input = format!(
            r#"
consume(require("./dep.js").default);
{mutation}
consume(require("./dep.js").default);
"#
        );
        let output = apply_unesm(&input);

        assert!(
            !output.contains("import ")
                && output.matches("require(\"./dep.js\").default").count() >= 2,
            "provider mutation must keep fresh default member reads:\n{output}"
        );
    }
}

#[test]
fn toplevel_require_default_member_does_not_reuse_later_default_prop() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").default);
var UIBase = require("./UIBase.js").default;
var keep = require("./keep.js");
"#;
    let expected = r#"
import defaultExport from "./UIBase.js";
import UIBase from "./UIBase.js";
import keep from "./keep.js";
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_does_not_reuse_mutated_default_prop() {
    let input = r#"
var UIBase = require("./UIBase.js").default;
UIBase = other;
(function (base) {
  use(base);
})(require("./UIBase.js").default);
var keep = require("./keep.js");
"#;
    let expected = r#"
import _UIBase from "./UIBase.js";
import defaultExport from "./UIBase.js";
import keep from "./keep.js";
var UIBase = _UIBase;
UIBase = other;
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_skips_written_binding_and_reuses_later_stable() {
    // Basename UIBase is taken and eval mentions `defaultExport`, so recovery is
    // only possible by reusing the later unwritten DefaultProp.
    let input = r#"
import { UIBase } from "./other.js";
eval("defaultExport");
var keep = require("./keep.js");
var poisoned = require("./UIBase.js").default;
poisoned = other;
var helper = require("./UIBase.js").default;
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert!(
        output.contains("import keep from \"./keep.js\"")
            && output.contains("import helper from \"./UIBase.js\"")
            && output.contains("})(helper)")
            && !output.contains("require(\"./UIBase.js\").default"),
        "a later unwritten DefaultProp must still be reusable after a mutated one:\n{output}"
    );
}

#[test]
fn toplevel_require_default_member_does_not_reuse_local_across_provider_member_write() {
    let input = r#"
var UIBase = require("./dep.js").default;
require("./dep.js").default = replacement;
consume(require("./dep.js").default);
"#;
    let expected = r#"
import UIBase from "./dep.js";
require("./dep.js").default = replacement;
consume(require("./dep.js").default);
"#;
    let output = apply_unesm(input);

    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_failure_does_not_block_default_member() {
    let input = r#"
import { A } from "./other.js";
var keep = require("./keep.js");
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(require("./B.js").default);
"#;
    let expected = r#"
import { A } from "./other.js";
import keep from "./keep.js";
import B from "./B.js";
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(B);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_failure_does_not_block_named_member() {
    let input = r#"
var keep = require("./keep.js");
require("./B.js").default = replacement;
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(require("./B.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import { A } from "./A.js";
require("./B.js").default = replacement;
(function (a) {
  use(a);
})(A);
(function (b) {
  use(b);
})(require("./B.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn stable_default_named_write_preserves_the_object_property() {
    let input = "class Engine {} module.exports = Engine; module.exports.value = next(); observe(Engine.value);";
    let output = render_pipeline_until(input, "UnEsm");
    assert!(output.contains("Engine.value = next()"), "{output}");
    assert_eq!(output.matches("next()").count(), 1, "{output}");
    assert!(output.contains("export const value ="), "{output}");
}

#[test]
fn stable_default_duplicate_named_writes_keep_property_effects() {
    let input = "class Engine {} module.exports = Engine; module.exports.value = first(); observe(Engine.value); module.exports.value = void 0;";
    let output = render_pipeline_until(input, "UnEsm");
    assert!(output.contains("Engine.value = first()"), "{output}");
    assert!(
        output.contains("Engine.value = void 0") || output.contains("Engine.value = undefined"),
        "{output}"
    );
}

#[test]
fn swc_async_runtime_require_preserves_its_namespace_export() {
    let input = r#"var helper = require("@swc/helpers/_/_async_to_generator"); consume(helper._(unknown));"#;
    assert_eq_normalized(
        &apply_unesm(input),
        r#"import * as helper from "@swc/helpers/_/_async_to_generator"; consume(helper._(unknown));"#,
    );
}

#[test]
fn swc_async_runtime_unsafe_namespace_uses_keep_the_commonjs_boundary() {
    for effect in [
        "helper = custom;",
        "var helper = custom;",
        "helper._ = custom;",
        "delete helper._;",
        "Object.defineProperty(helper, '_', { value: custom });",
        "consume(helper);",
        "with (scope) { observe(); }",
        "eval(code);",
        "helper(unknown);",
    ] {
        let input = format!(
            r#"var helper = require("@swc/helpers/_/_async_to_generator"); {effect} exports.load = function() {{ return helper._(unknown); }};"#
        );
        assert_eq_normalized(&apply_unesm(&input), &input);
    }
}

#[test]
fn swc_async_runtime_namespace_conversion_requires_exact_unresolved_require() {
    for input in [
        r#"function require(path) { return custom; } var helper = require("@swc/helpers/_/_async_to_generator"); consume(helper._);"#,
        r#"var helper = require("@swc/helpers/_/_async_to_generator/extra"); consume(helper._);"#,
        r#"var helper = require("@swc/helpers/_/_async_to_generator", extra); consume(helper._);"#,
    ] {
        assert!(!apply_unesm(input).contains("import * as helper"));
    }
}

#[test]
fn esm_only_modules_keep_import_ordering_and_export_cleanup() {
    let input = r#"
import { first } from "a";
use(first);
import { second } from "b";
import { third } from "a";
var value = makeValue();
export { value };
var alias = value;
export default alias;
"#;
    let expected = r#"
import { first } from "a";
import { third } from "a";
import { second } from "b";
use(first);
export var value = makeValue();
export default value;
"#;
    let output = common::render_rule(input, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn chained_export_initializers_do_not_duplicate_recovered_bindings() {
    for root in ["exports", "module.exports"] {
        for value in ["void 0", "undefined"] {
            for count in [2, 3, 8] {
                let names: Vec<_> = (0..count).map(|index| format!("value{index}")).collect();
                let chain = names
                    .iter()
                    .map(|name| format!("{root}.{name} = "))
                    .collect::<String>();
                let declarations = names
                    .iter()
                    .map(|name| format!("const {name} = () => 1; {root}.{name} = {name};"))
                    .collect::<String>();
                let source = format!("{chain}{value}; {declarations} function dynamic(code) {{ return eval(code); }}");
                let once = common::render_pipeline_between(&source, "UnCurlyBraces", "UnEsm");
                let twice = common::render_pipeline_between(&once, "UnCurlyBraces", "UnEsm");
                assert_eq_normalized(&once, &twice);
                assert!(
                    !once.contains("exports."),
                    "initializer must be fully consumed: {once}"
                );
                let findings = validate_output_modules(&[("entry.js".into(), twice.clone())]);
                assert!(findings.is_empty(), "{findings:?}\n{twice}");
                for name in &names {
                    assert!(
                        twice.contains(&format!("export const {name} =")),
                        "missing value: {twice}"
                    );
                }
            }
        }
    }
}

#[test]
fn chained_export_initializers_preserve_effectful_and_shadowed_values() {
    for source in [
        "exports.left = exports.right = void sideEffect();",
        "const undefined = sideEffect(); exports.left = exports.right = undefined;",
        "const exports = {}; exports.left = exports.right = void 0;",
        "const module = { exports: {} }; module.exports.left = module.exports.right = void 0;",
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq!(
            output.matches("sideEffect()").count(),
            source.matches("sideEffect()").count(),
            "{output}"
        );
        if source.starts_with("const exports") || source.starts_with("const module") {
            assert!(
                !output.contains("export const"),
                "local objects are not CommonJS: {output}"
            );
        }
    }
}

#[test]
fn conditional_named_exports_become_live_bindings() {
    for root in ["exports", "module.exports"] {
        let source = r#"
if (flag) {
  $ROOT.current = first();
} else {
  $ROOT.current = second();
}
function read() {
  return $ROOT.current();
}
function reset() {
  $ROOT.current = third();
  return $ROOT.current;
}
exports.snapshot = read;
"#
        .replace("$ROOT", root);
        let expected = r#"
export var current;
if (flag) {
  current = first();
} else {
  current = second();
}
function read() {
  return current();
}
function reset() {
  current = third();
  return current;
}
export { read as snapshot };
"#;

        let output = common::render_rule(&source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, expected);
    }
}

#[test]
fn leading_conditional_export_sentinels_are_removed() {
    let source = r#"
"use strict";
import dependency from "./dependency.js";
function readDependency() {
  return dependency;
}
exports.current = void 0;
module.exports.ready = undefined;
if (flag) {
  exports.current = first();
  module.exports.ready = true;
} else {
  exports.current = second();
  module.exports.ready = false;
}
consume(readDependency(), exports.current, module.exports.ready);
"#;
    let expected = r#"
import dependency from "./dependency.js";
export var current;
export var ready;
"use strict";
function readDependency() {
  return dependency;
}
if (flag) {
  current = first();
  ready = true;
} else {
  current = second();
  ready = false;
}
consume(readDependency(), current, ready);
"#;

    let output = common::render_rule(source, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);

    let pipeline_output = common::render_pipeline(source);
    assert!(
        pipeline_output.contains("export let current;"),
        "{pipeline_output}"
    );
    assert!(
        pipeline_output.contains("export let ready;"),
        "{pipeline_output}"
    );
    assert!(
        !pipeline_output.contains("current = undefined"),
        "{pipeline_output}"
    );
    assert!(
        !pipeline_output.contains("ready = undefined"),
        "{pipeline_output}"
    );
    assert!(
        validate_output_modules(&[
            ("entry.js".into(), pipeline_output.clone()),
            ("dependency.js".into(), "export default {};".into()),
        ])
        .is_empty(),
        "{pipeline_output}"
    );
}

#[test]
fn require_declarations_do_not_end_the_export_sentinel_prefix() {
    let source = r#"
const dependency = require("dependency");
exports.current = void 0;
if (flag) {
  exports.current = dependency.first();
} else {
  exports.current = dependency.second();
}
"#;
    let expected = r#"
import dependency from "dependency";
export var current;
if (flag) {
  current = dependency.first();
} else {
  current = dependency.second();
}
"#;

    let output = common::render_rule(source, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn conditional_export_sentinels_after_other_code_are_preserved() {
    for prefix in [
        "sideEffect();",
        "require('side-effect');",
        "const marker = 1;",
        "exports.current = initial;",
    ] {
        let source = format!(
            "{prefix} exports.current = void 0; if (flag) {{ exports.current = next(); }} consume(exports.current);"
        );
        let output = common::render_rule(&source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert!(
            output.contains("current = void 0") || output.contains("current = undefined"),
            "{output}"
        );
    }

    let shadowed = r#"
const undefined = fallback;
exports.current = undefined;
if (flag) {
  exports.current = next();
}
consume(exports.current);
"#;
    let output = common::render_rule(shadowed, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    // The shadowed `undefined` is a value, not a sentinel: its initializer
    // reaches the export.
    assert!(output.contains("current = fallback"), "{output}");

    let effectful_void = r#"
exports.current = void sideEffect();
if (flag) {
  exports.current = next();
}
consume(exports.current);
"#;
    let output = common::render_rule(effectful_void, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq!(output.matches("sideEffect()").count(), 1, "{output}");
    assert!(output.contains("current = void sideEffect()"), "{output}");
}

#[test]
fn conditional_named_export_chains_share_live_bindings() {
    let source = r#"
if (flag) {
  exports.left = module.exports.right = makeValue();
}
consume(exports.left, module.exports.right);
"#;
    let expected = r#"
export var left;
export var right;
if (flag) {
  left = right = makeValue();
}
consume(left, right);
"#;

    let output = common::render_rule(source, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn conditional_named_export_bindings_do_not_capture_existing_names() {
    let source = r#"
const current = local;
if (flag) {
  exports.current = next();
}
consume(current, exports.current);
"#;
    let expected = r#"
var _current;
export { _current as current };
const current = local;
if (flag) {
  _current = next();
}
consume(current, _current);
"#;

    let output = common::render_rule(source, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn conditional_named_exports_keep_the_commonjs_boundary_when_the_module_gate_fails() {
    for source in [
        "const alias = exports; if (flag) { exports.value = 1; } exports.ready = 1;",
        "module.exports = replacement; if (flag) { exports.value = 1; } exports.ready = 1;",
        "if (flag) { exports[key] = 1; } exports.ready = 1;",
        "if (flag) { delete exports.value; } exports.ready = 1;",
        "if (flag) { exports.value = 1; } eval('value'); exports.ready = 1;",
        "if (flag) { exports.value = 1; } with (scope) { observe(); } exports.ready = 1;",
        "Object.defineProperty(exports, 'value', { value: 0 }); if (flag) { exports.value = 1; } exports.ready = 1;",
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, source);
    }
}

/// Once the `exports` binding is reassigned or aliased, a static
/// `exports.x` access no longer proves which object it touches, so no part of
/// the module may be converted.
#[test]
fn rebound_or_aliased_exports_keeps_the_commonjs_boundary() {
    for source in [
        "exports.a = 1; exports = { b: 2 }; exports.c = 3; module.exports.d = 4;",
        "exports.a = 1; function reset() { exports = { b: 2 }; } exports.c = 3;",
        "exports.a = 1; [exports] = [other];",
        "var alias = exports; alias.a = 1; exports.b = 2;",
        "exports.a = 1; function current() { return exports; }",
        "exports.a = 1; holder = { exports };",
        // The Node idiom that replaces the exported object and keeps the
        // binding pointing at it.
        "function Parser() {} exports = module.exports = Parser; exports.Parser = Parser;",
        "function Parser() {} module.exports = exports = Parser; exports.Parser = Parser;",
        "(function (root) { function Parser() {} if (typeof exports !== 'undefined') { if (typeof module !== 'undefined' && module.exports) { exports = module.exports = Parser; } exports.Parser = Parser; } else { root.Parser = Parser; } })(this);",
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, source);
    }
}

/// Passing `exports` to a call does not fail the gate, because recognized
/// helpers that take it (`__exportStar`, `require.d`) are recovered. When the
/// call survives conversion, the ES module would throw on it, so the module
/// stays CommonJS.
#[test]
fn surviving_exports_call_arguments_keep_the_commonjs_boundary() {
    for source in [
        "register(exports);\nexports.a = 1;\n",
        "__exportStar(require(12345), exports);\nexports.own = 1;\n",
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, source);
    }

    // TypeScript's helper with a module id the unpacker could not resolve.
    let source = r#"
"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (p !== "default" && !Object.prototype.hasOwnProperty.call(exports, p)) __createBinding(exports, m, p);
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.own = void 0;
__exportStar(require(12345), exports);
exports.own = 1;
"#;
    let output = render_pipeline(source);
    assert!(
        output.contains("__exportStar(require(12345), exports);"),
        "{output}"
    );
    assert!(output.contains("exports.own = 1;"), "{output}");
    assert!(
        !output
            .lines()
            .any(|line| line.starts_with("export ") || line.starts_with("import ")),
        "{output}"
    );

    // A recognized helper call is still recovered.
    let output = render_pipeline(&source.replace("require(12345)", "require(\"./dep\")"));
    assert!(output.contains("export * from \"./dep\";"), "{output}");
    assert!(!output.contains("exports"), "{output}");
}

/// rollup's `Symbol.toStringTag` marker passes `exports` to a call, so it
/// must be gone before conversion or the module would stay CommonJS.
#[test]
fn rollup_to_string_tag_markers_do_not_keep_commonjs() {
    // rollup 4.63, `format: "cjs"`, `generatedCode.symbols`.
    for input in [
        "'use strict';\n\nObject.defineProperty(exports, Symbol.toStringTag, { value: 'Module' });\n\nconst a = 1;\nexports.b = 2;\nfunction setB(v) { exports.b = v; }\n\nexports.a = a;\nexports.setB = setB;\n",
        "'use strict';\n\nObject.defineProperties(exports, { __esModule: { value: true }, [Symbol.toStringTag]: { value: 'Module' } });\n\nconst a = 1;\nfunction f() { return a; }\n\nexports.a = a;\nexports.default = f;\n",
    ] {
        let output = render_pipeline(input);
        assert!(!output.contains("exports"), "{output}");
        assert!(!output.contains("toStringTag"), "{output}");
        assert!(output.contains("export"), "{output}");
    }
}

/// A top-level `this` in CommonJS is `module.exports`; in ESM it is
/// `undefined`. Converting would make writes throw and reads change value.
#[test]
fn top_level_this_keeps_the_commonjs_boundary() {
    for source in [
        "this.value = 1; exports.other = 2;",
        "var root = typeof self == 'object' ? self : this; exports.root = root;",
        "(function (root) { root.ready = true; })(this); exports.other = 2;",
        "exports.a = 1; observe(typeof this);",
        // No export at all: the require alone would become an import.
        "var dep = require('dep'); this.value = dep;",
        // Arrows, a class heritage, and a computed class key see the outer `this`.
        "const read = () => this.value; exports.read = read;",
        "class Child extends (this.Base || Object) {} exports.Child = Child;",
        "class Keyed { [this.key]() {} } exports.Keyed = Keyed;",
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, source);
    }
}

#[test]
fn this_bound_by_a_function_or_class_body_does_not_keep_commonjs() {
    for source in [
        "function self() { return this; } exports.self = self;",
        "exports.object = { get value() { return this.v; }, method() { return this.v; } };",
        "class Holder { constructor() { this.a = 1; } b = this.a; static { this.c = 1; } read() { return this.a; } } exports.Holder = Holder;",
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert!(output.contains("export"), "{output}");
        assert!(!output.contains("exports"), "{output}");
    }
}

/// TypeScript declares each helper as `(this && this.__name) || impl`. The
/// guard reads `undefined` in ESM and an absent property of the empty
/// `module.exports` in CommonJS, so both pick `impl`.
#[test]
fn typescript_helper_guards_on_this_do_not_keep_commonjs() {
    // TypeScript 5.9, `module: CommonJS`, `esModuleInterop`, target ES2015.
    let source = r#"
"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
const dep_1 = __importDefault(require("./dep"));
function run() {
    return __awaiter(this, void 0, void 0, function* () { return (0, dep_1.default)(); });
}
"#;
    let output = render_pipeline(source);
    assert!(output.contains("from \"./dep\""), "{output}");
    assert!(output.contains("export { run }"), "{output}");
    assert!(!output.contains("exports"), "{output}");
    assert!(!output.contains("this"), "{output}");
}

/// Only the TypeScript helper guard shape is exempt: a guard whose property
/// the module also exports, or a non-helper name, still reads
/// `module.exports`.
#[test]
fn other_this_guards_keep_the_commonjs_boundary() {
    for source in [
        "var helper = this && this.helper || fallback; exports.a = helper;",
        "exports.__assign = mine; var __assign = this && this.__assign || fallback; exports.b = __assign;",
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, source);
    }
}

#[test]
fn property_storage_recovers_compound_deferred_and_read_only_names() {
    for (source, expected) in [
        (
            "if (flag) { exports.value += 1; } exports.ready = 1;",
            "export var value; if (flag) { value += 1; } export const ready = 1;",
        ),
        (
            "if (flag) { exports.value = 1; } function later() { exports.other = 2; } exports.ready = 1;",
            "export var value; export var other; if (flag) { value = 1; } function later() { other = 2; } export const ready = 1;",
        ),
        // A name that is only read was never an export; it stays a local.
        (
            "if (flag) { exports.value = 1; } observe(exports.other); exports.ready = 1;",
            "export var value; var other; if (flag) { value = 1; } observe(other); export const ready = 1;",
        ),
        (
            "if (flag) { exports.current = first(); } else { exports.current = second(); } consume(exports?.current);",
            "export var current; if (flag) { current = first(); } else { current = second(); } consume(current);",
        ),
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&output, expected);
    }
}

#[test]
fn mutable_named_export_values_are_snapshots() {
    let source = r#"
exports.a = 1;
if (flag) {
  exports.a = 2;
}
exports.b = exports.a;
function bump() {
  exports.a = 3;
}
exports.bump = bump;
"#;
    let expected = r#"
export var a = 1;
if (flag) {
  a = 2;
}
export const b = a;
function bump() {
  a = 3;
}
export { bump };
"#;

    let output = common::render_rule(source, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);
    let pipeline_output = common::render_pipeline(source);
    assert!(
        pipeline_output.contains("export const b = a;"),
        "{pipeline_output}"
    );
    assert!(
        validate_output_modules(&[("entry.js".into(), pipeline_output.clone())]).is_empty(),
        "{pipeline_output}"
    );

    let before_write = r#"
exports.b = exports.a;
if (flag) {
  exports.a = 1;
}
"#;
    let expected = r#"
export var a;
export const b = a;
if (flag) {
  a = 1;
}
"#;
    let output = common::render_rule(before_write, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);

    let same_name = r#"
var value = 1;
exports.value = value;
function bump() {
  value = 2;
}
"#;
    let expected = r#"
var value = 1;
var _value = value;
export { _value as value };
function bump() {
  value = 2;
}
"#;
    let output = common::render_rule(same_name, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn deferred_named_export_writes_use_property_storage() {
    let source = r#"
class Example {
  value = exports.field = createField();
  constructor() {
    exports.value = createValue();
  }
}
"#;

    let expected = r#"
export var field;
export var value;
class Example {
  value = field = createField();
  constructor() {
    value = createValue();
  }
}
"#;

    let output = common::render_rule(source, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn conditional_named_exports_validate_through_the_pipeline() {
    let source = r#"
var selected;
if (flag) {
  selected = exports.current = first();
} else {
  exports.current = second();
  selected = exports.current;
}
consume(selected, exports.current);
"#;
    let output = common::render_pipeline(source);

    assert!(output.contains("export let current;"), "{output}");
    assert!(!output.contains("exports."), "{output}");
    assert!(
        validate_output_modules(&[("entry.js".into(), output.clone())]).is_empty(),
        "{output}"
    );
}

#[test]
fn unsupported_named_export_chains_keep_the_commonjs_boundary() {
    for chain in [
        "module.exports.a = module.exports = void 0;",
        "module.exports = module.exports.default = function() { return marker; };",
        "module.exports = exports.default = exports.a = makeValue();",
        "exports.a = exports.__esModule = true;",
        "exports.a = module.exports.a = makeValue();",
        "exports[key()] = exports.b = void 0;",
        "exports.a = exports.b = local = makeValue();",
        "exports.a = local = makeValue();",
        "const initial = exports.a = exports.b = makeValue();",
        "if (flag) { exports.a = exports.b = makeValue(); }",
        "class Holder { static { exports.a = exports.b = makeValue(); } }",
    ] {
        let source = format!("const dep = require('dep'); {chain} exports.ready = dep; function dynamic(code) {{ return eval(code); }}");
        let once = common::render_rule(&source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&once, &source);
        let twice = common::render_rule(&once, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&twice, &once);
    }
}

#[test]
fn local_assignment_tails_keep_existing_export_recovery() {
    for source in [
        "var value; exports.item = value = makeValue();",
        "var Item = build(); exports.Item = Item; exports.Item = Item = decorate(Item);",
        "exports.Mode = void 0; var Mode; (function(Mode) { Mode[Mode.On = 1] = 'On'; })(Mode || (exports.Mode = Mode = {}));",
    ] {
        let output = common::render_pipeline(source);
        assert!(output.contains("export "), "{output}");
        assert!(!output.contains("exports."), "{output}");
        assert!(validate_output_modules(&[("entry.js".into(), output.clone())]).is_empty(), "{output}");
        assert_eq!(output.matches("makeValue()").count(), source.matches("makeValue()").count());
        assert_eq!(output.matches("decorate(").count(), source.matches("decorate(").count());
    }
}

#[test]
fn whole_named_export_chains_are_recovered_in_one_pass() {
    for (source, expected) in [
        (
            "exports.a = exports.b = function () { return 1; };",
            "export var b = function () { return 1; }; export { b as a };",
        ),
        (
            "exports.left = exports.right = () => 2;",
            "export var right = () => 2; export { right as left };",
        ),
        (
            "exports.a = module.exports.b = void 0; const a = 1; exports.a = a; const b = 2; module.exports.b = b;",
            "export const a = 1; export const b = 2;",
        ),
        (
            "module.exports = exports.default = function () { return 1; };",
            "export default function () { return 1; };",
        ),
        (
            "module.exports = exports.helper = function () { return 1; };",
            "export var helper = function () { return 1; }; export default helper;",
        ),
        (
            "module.exports = module.exports.flag = 1;",
            "export const flag = 1; export default 1;",
        ),
        (
            "var u; u = exports.paint = () => {}; use(u);",
            "var u; export var paint = () => {}; u = paint; use(u);",
        ),
        (
            "let u; u = exports.paint = 1; use(u);",
            "let u; export const paint = 1; u = 1; use(u);",
        ),
        (
            "const b = 1; exports.a = exports.b = () => b;",
            "const b = 1; var a = () => b; export { a as b }; export { a };",
        ),
        (
            "exports.foo = exports.default = function () {};",
            "var foo = function () {}; export default foo; export { foo };",
        ),
        (
            "exports.decode = exports.parse = require(\"qs-decode\"); exports.encode = exports.stringify = require(\"qs-encode\");",
            "import parse from \"qs-decode\"; import stringify from \"qs-encode\"; export { parse }; export { parse as decode }; export { stringify }; export { stringify as encode };",
        ),
        (
            // The mirror pair keeps the existing `export default require(...)` re-export shape.
            "module.exports = exports.default = require(\"impl-lib\");",
            "export default require(\"impl-lib\");",
        ),
        // A call on a provider binding (a top-level `require("literal")`
        // declarator that is never rewritten) with repeatable arguments is
        // evaluated once into a binding at the chain's position
        // (`chain_receiver_reference_order`).
        (
            "var Lib = require(\"./lib\"); var KEY = \"alpha\"; exports.first = exports.second = Lib.matcher(KEY);",
            "import Lib from \"./lib\"; var KEY = \"alpha\"; export var second = Lib.matcher(KEY); export { second as first };",
        ),
        (
            "var Lib = require(\"./lib\"); exports.first = exports.second = Lib.matcher(\"beta\");",
            "import Lib from \"./lib\"; export var second = Lib.matcher(\"beta\"); export { second as first };",
        ),
        (
            "var counter = require(\"./lib\").matcher; var T = \"gamma\"; exports.first = exports.second = counter(T);",
            "import { matcher as counter } from \"./lib\"; var T = \"gamma\"; export var second = counter(T); export { second as first };",
        ),
        (
            "var make = require(\"./make\"); exports.a = exports.b = make(1, void 0);",
            "import make from \"./make\"; export var b = make(1, void 0); export { b as a };",
        ),
        (
            "var P = require(\"./lib\"); var Q = P; exports.a = exports.b = Q.make(1);",
            "import P from \"./lib\"; var Q = P; export var b = Q.make(1); export { b as a };",
        ),
        (
            "var Lib = require(\"./lib\"); module.exports = exports.helper = Lib.build.helper();",
            "import Lib from \"./lib\"; export var helper = Lib.build.helper(); export default helper;",
        ),
    ] {
        let once = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&once, expected);
        let twice = common::render_rule(&once, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&twice, &once);
    }
}

#[test]
fn named_export_chains_keep_the_boundary_under_dynamic_scope_or_receiver_replacement() {
    for source in [
        // The recovered exports become module bindings that direct eval or
        // `with` could observe.
        "exports.a = exports.b = () => {}; function dynamic(code) { return eval(code); }",
        "exports.a = module.exports.b = 1; eval('a');",
        "module.exports = exports.default = fn; function dynamic(code) { return eval(code); }",
        "with (scope) { exports.a = exports.b = 1; }",
        // Passing a wrapper binding to a call can replace or leak the object.
        "var P = require(\"./lib\"); P = function () { module.exports = {}; return 1; }; var Q = P; module.exports.a = module.exports.b = Q();",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib.make(module);",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib.make(exports);",
        "module.exports.a = module.exports.b = (module.exports = {}, 1);",
        "exports.a = exports.b = (exports = {}, 1);",
        "module.exports = exports.default = (exports = {}, fn);",
        "exports.a = exports.b = class { static { module.exports = {}; } };",
        // `module.exports === module` makes the exports key replace the slot.
        "module.exports.a = module.exports.exports = void 0;",
        "exports.a = exports.exports = 1;",
    ] {
        let once = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert_eq_normalized(&once, source);
    }
}

#[test]
fn property_storage_recovers_named_export_chains_with_effectful_values() {
    // Every link writes a property-storage name, so the chain becomes one
    // assignment chain of module bindings with the same single evaluation.
    for source in [
        "exports.a = exports.b = makeValue();",
        "exports.a = exports.b = require(name);",
        "exports.a = exports.b = require(\"x\", extra);",
        "exports.a = exports.b = require(...specs);",
        "const require = load; exports.a = exports.b = require(\"x\");",
        "exports.a = exports.b = new Thing();",
        "exports.a = exports.b = void sideEffect();",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib.make(other());",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib[key](1);",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib.make(...xs);",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib?.make(1);",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib.make?.(1);",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib.make(require);",
        "var Lib = require(\"./lib\"); exports.a = exports.b = Lib.make(/re/);",
        "var Lib = require(\"./lib\"); exports.a = exports.b = new Lib.Thing(1);",
        "var Lib = require(\"./lib\"); Lib = local; exports.a = exports.b = Lib.make(1);",
        "var Lib = require(\"./lib\"); var Lib = other; exports.a = exports.b = Lib.make(1);",
        "var Lib = require(\"./lib\"); function reset() { Lib = other; } exports.a = exports.b = Lib.make(1);",
        "var P = require(\"./lib\"); var P = other; var Q = P; exports.a = exports.b = Q(1);",
        "var P = require(\"./lib\"); var Q = P.make; function swap() { P = other; } exports.a = exports.b = Q(1);",
        "var P = require(\"./lib\"); var Q = P; Q = local; exports.a = exports.b = Q(1);",
        "var Lib = require(name); exports.a = exports.b = Lib.make(1);",
        "var Lib = require(\"./lib\", extra); exports.a = exports.b = Lib.make(1);",
        "var Lib = require(\"./lib\")[pick]; exports.a = exports.b = Lib.make(1);",
        "var Lib = require(\"./lib\")(); exports.a = exports.b = Lib.make(1);",
        "function Lib() {} exports.a = exports.b = Lib.make(1);",
        "var Lib = { make() {} }; exports.a = exports.b = Lib.make(1);",
        "function scope() { var Lib = require(\"./lib\"); } exports.a = exports.b = Lib.make(1);",
        "const require = load; var Lib = require(\"./lib\"); exports.a = exports.b = Lib.make(1);",
    ] {
        let output = common::render_rule(source, |mark| {
            wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
        });
        assert!(output.contains("a = b = "), "{source}\n{output}");
        assert!(!output.contains("exports"), "{source}\n{output}");
        assert!(
            !validate_output_modules(&[("entry.js".into(), output.clone())])
                .iter()
                .any(|finding| finding.kind == OutputFindingKind::DuplicateDeclaration),
            "{source}\n{output}"
        );
    }
}

#[test]
fn whole_named_export_chains_validate_through_the_pipeline() {
    for source in [
        "exports.a = exports.b = function () { return 1; };",
        "module.exports = exports.default = function () { return 1; };",
        "var u; if (flag) { u = () => 1; } u = exports.paint = () => {}; exports.now = () => 1;",
        "exports.a = module.exports.b = void 0; const a = () => 1; exports.a = a; var b = () => 2; module.exports.b = b;",
        "exports.decode = exports.parse = require(\"qs-decode\"); exports.encode = exports.stringify = require(\"qs-encode\");",
        "var Lib = require(\"shared-lib\"); var KEY = \"alpha\"; exports.name = KEY; exports.root = KEY; exports.first = exports.second = Lib.matcher(KEY); exports.run = function (input) { return Lib.run(input, KEY); };",
        "var counter = require(\"shared-lib\").matcher; var T = \"gamma\"; exports.first = exports.second = counter(T);",
    ] {
        let output = common::render_pipeline(source);
        assert!(!output.contains("exports."), "{output}");
        assert!(output.contains("export "), "{output}");
        assert!(
            validate_output_modules(&[("entry.js".into(), output.clone())]).is_empty(),
            "{output}"
        );
    }
}

// ============================================================
// Export storage model (report only)
// ============================================================

/// `name=storage(binding)` per export name, in first-access order, or the
/// module gate failure.
fn export_storage(input: &str) -> Vec<String> {
    let report = wakaru_core::explain_commonjs_exports(input, Default::default())
        .expect("single-file input analyzes");
    if let Some(gate) = report.gate {
        return vec![format!("gate: {gate}")];
    }
    report
        .exports
        .iter()
        .map(|export| match &export.binding {
            Some(binding) => format!("{}={}({binding})", export.name, export.storage),
            None => format!("{}={}", export.name, export.storage),
        })
        .collect()
}

fn export_rejections(input: &str, name: &str) -> Vec<String> {
    let report = wakaru_core::explain_commonjs_exports(input, Default::default())
        .expect("single-file input analyzes");
    report
        .exports
        .into_iter()
        .find(|export| export.name == name)
        .map(|export| export.rejected)
        .unwrap_or_default()
}

#[test]
fn export_storage_property_writes_in_function_bodies() {
    let input = r#"
exports.count = void 0;
exports.count = 0;
exports.bump = bump;
function bump() { exports.count += 1; return exports.count < exports.limit; }
exports.limit = compute();
"#;
    assert_eq!(
        export_storage(input),
        ["count=property", "bump=mirror(bump)", "limit=property"]
    );
}

#[test]
fn export_storage_mirror_chains_in_declaration_and_assignment() {
    let input = r#"
exports.count = void 0;
let count = exports.count = start();
function inc() {
  exports.count = count = count + 1;
  return count;
}
exports.inc = inc;
"#;
    assert_eq!(
        export_storage(input),
        ["count=mirror(count)", "inc=mirror(inc)"]
    );
}

#[test]
fn export_storage_mirror_may_follow_through_other_mirror_statements() {
    let input = r#"
let [first, second] = load();
exports.second = second;
exports.first = first;
function swap() {
  [first, second] = [second, first];
  exports.first = first, exports.second = second;
}
exports.swap = swap;
"#;
    assert_eq!(
        export_storage(input),
        [
            "second=mirror(second)",
            "first=mirror(first)",
            "swap=mirror(swap)"
        ]
    );
}

#[test]
fn export_storage_lagging_copy_is_property_storage() {
    // The property keeps the first value while `n` moves on: a live export of
    // `n` would change what importers see.
    let input = r#"
let n = start();
exports.n = n;
exports.step = function () { n = n + 1; };
"#;
    assert_eq!(export_storage(input), ["n=property", "step=property"]);
    assert!(
        export_rejections(input, "n")[0].starts_with("mirror: a write of `n` is not mirrored"),
        "{:?}",
        export_rejections(input, "n")
    );
}

#[test]
fn export_storage_mirror_must_be_adjacent_when_the_property_is_read() {
    let input = r#"
let n = start();
log(exports.n);
exports.n = n;
"#;
    assert_eq!(export_storage(input), ["n=property"]);
}

#[test]
fn export_storage_write_inside_expression_arrow_is_not_mirrored_by_its_statement() {
    let input = r#"
let n = start();
const set = (v) => (n = v);
exports.n = n;
exports.set = set;
"#;
    assert_eq!(export_storage(input), ["n=property", "set=mirror(set)"]);
}

#[test]
fn export_storage_final_copy_of_unread_property_is_a_mirror() {
    // Rollup places every copy at the end of the module.
    let input = r#"
const limit = compute();
function twice(v) { return v * limit; }
exports.limit = limit;
exports.twice = twice;
"#;
    assert_eq!(
        export_storage(input),
        ["limit=mirror(limit)", "twice=mirror(twice)"]
    );
}

#[test]
fn export_storage_copied_parameter_is_not_a_mirror_candidate() {
    let input = r#"
exports.level = compute();
function setLevel(v) { exports.level = v; }
exports.setLevel = setLevel;
"#;
    assert_eq!(
        export_storage(input),
        ["level=property", "setLevel=mirror(setLevel)"]
    );
    assert!(export_rejections(input, "level").is_empty());
}

#[test]
fn export_storage_getters() {
    let input = r#"
var dep = require("dep");
let live = compute();
Object.defineProperty(exports, "live", { enumerable: true, get: function () { return live; } });
Object.defineProperty(exports, "other", { enumerable: true, get: function () { return dep.other; } });
function bump() { live = live + 1; }
exports.bump = bump;
"#;
    assert_eq!(
        export_storage(input),
        [
            "live=getter(live)",
            "other=getter(dep.other)",
            "bump=mirror(bump)"
        ]
    );
}

#[test]
fn export_storage_getter_with_a_write_is_unrecovered() {
    let input = r#"
let live = compute();
Object.defineProperty(exports, "live", { enumerable: true, get: function () { return live; } });
exports.live = other();
"#;
    assert_eq!(export_storage(input), ["live=unrecovered"]);
}

#[test]
fn export_storage_receiver_sensitive_call_is_unrecovered() {
    let input = r#"
exports.run = function () { return this.state; };
exports.check = () => 1;
exports.run();
exports.check();
"#;
    assert_eq!(export_storage(input), ["run=unrecovered", "check=property"]);
}

#[test]
fn export_storage_module_gates() {
    for (input, gate) in [
        (
            "exports.a = 1; register(exports);",
            "`exports` is passed to a call",
        ),
        (
            "exports.a = 1; var alias = exports;",
            "`exports` is used as a value",
        ),
        ("exports.a = 1; exports[key] = 2;", "computed `exports` key"),
        (
            "exports.a = 1; module.exports = other;",
            "`module.exports` is used as a value",
        ),
        ("exports.a = 1; exports = other;", "`exports` is reassigned"),
        ("exports.a = 1; module[key] = 2;", "computed `module` key"),
        (
            "exports.a = 1; delete exports.a;",
            "`delete` of an `exports` property",
        ),
    ] {
        let storage = export_storage(input);
        assert!(
            storage.len() == 1 && storage[0].starts_with(&format!("gate: {gate}")),
            "{input}: {storage:?}"
        );
    }
    assert!(export_storage("const a = 1; export { a };").is_empty());
    // `module.exports.name` is the same object as `exports.name`.
    assert_eq!(
        export_storage("exports.a = 1; module.exports.b = 2; module.hot && module.hot.accept();"),
        ["a=property", "b=property"]
    );
}

// ============================================================
// Property storage recovery
// ============================================================

#[test]
fn property_storage_recovers_typescript_writes_in_function_bodies() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.limit = exports.count = void 0;
exports.bump = bump;
exports.count = 0;
function bump() {
  exports.count += 1;
  return exports.count < exports.limit;
}
exports.limit = 3;
"#;
    let output = render_pipeline(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(output.contains("export let count = 0;"), "{output}");
    assert!(output.contains("count += 1;"), "{output}");
    assert!(output.contains("return count < limit;"), "{output}");
    assert!(
        validate_output_modules(&[("entry.js".into(), output.clone())]).is_empty(),
        "{output}"
    );
}

#[test]
fn property_storage_takes_over_a_seed_local() {
    let input = r#"
let n = 0;
exports.n = n;
function step() { exports.n++; }
exports.step = step;
"#;
    let expected = r#"
export let n = 0;
function step() {
  n++;
}
export { step };
"#;
    assert_eq_normalized(&apply(input), expected);

    // A local that is used again stays, and the export gets its own binding.
    let used_again = r#"
let n = 0;
exports.n = n;
log(n);
function step() { exports.n++; }
"#;
    let output = apply(used_again);
    assert!(output.contains("let _n = n;"), "{output}");
    assert!(output.contains("_n++;"), "{output}");
    assert!(output.contains("log(n);"), "{output}");
}

#[test]
fn property_storage_avoids_names_that_would_be_shadowed() {
    let input = r#"
exports.count = 0;
function setCount(count) { exports.count = count; }
function read() { return exports.count; }
exports.setCount = setCount;
exports.read = read;
"#;
    let output = apply(input);
    assert!(output.contains("_count = count;"), "{output}");
    assert!(output.contains("return _count;"), "{output}");
    assert!(output.contains("_count as count"), "{output}");
}

#[test]
fn property_storage_exports_string_names() {
    let input = r#"
exports["a-b"] = compute();
function bump() { exports["a-b"] += 1; }
exports.bump = bump;
"#;
    let output = apply(input);
    assert!(output.contains("_a_b += 1;"), "{output}");
    assert!(output.contains(r#"_a_b as "a-b""#), "{output}");
    assert!(!output.contains("exports"), "{output}");
}

#[test]
fn property_storage_rewrites_calls_through_reassigned_values() {
    // call_receiver_independence: the `exports` receiver of `exports.f()` is
    // an artifact of lowering `f()`.
    let input = r#"
exports.current = first();
function read() { return exports.current(); }
function reset() { exports.current = second(); }
exports.read = read;
exports.reset = reset;
"#;
    let output = apply(input);
    assert!(output.contains("return current();"), "{output}");
    assert!(output.contains("current = second();"), "{output}");
    assert!(!output.contains("exports"), "{output}");
}

#[test]
fn export_getter_inside_a_function_fails_the_storage_gate() {
    // A later recovery unwraps the factory IIFE; its `exports` accesses must
    // reach it unchanged.
    let input = r#"
((t) => {
  require.d(exports, "VERSION", () => o);
  const r = t.make();
  exports.default = r;
  const o = "1.0.0";
})(require("./dependency.js"));
"#;
    let storage = export_storage(input);
    assert!(
        storage.len() == 1
            && storage[0].starts_with("gate: an export getter is defined inside a function"),
        "{storage:?}"
    );
    let output = common::render_rule(input, |mark| {
        wakaru_core::rules::UnEsm::new(mark, RewriteLevel::Standard)
    });
    assert!(output.contains("exports.default = r"), "{output}");
}

#[test]
fn export_sentinels_after_typescript_helper_declarations_are_removed() {
    let input = r#"
"use strict";
var __awaiter = this && this.__awaiter || function (thisArg, body) { return body(); };
exports.endpoint = void 0;
exports.endpoint = "https://example.com/report";
function report() { return exports.endpoint; }
exports.report = report;
"#;
    let output = apply(input);
    assert!(!output.contains("endpoint = undefined"), "{output}");
    assert!(
        output.contains("endpoint = \"https://example.com/report\""),
        "{output}"
    );

    // A call before the sentinel may write the property, so the sentinel
    // stays.
    let after_call = r#"
setup();
exports.endpoint = void 0;
exports.endpoint = compute();
function setup() { exports.endpoint = 1; }
function report() { return exports.endpoint; }
"#;
    let output = apply(after_call);
    assert!(output.contains("endpoint = undefined"), "{output}");
}

// ============================================================
// Mirror storage recovery
// ============================================================

fn assert_valid_esm(output: &str) {
    assert!(
        validate_output_modules(&[("entry.js".into(), output.to_string())]).is_empty(),
        "{output}"
    );
}

#[test]
fn mirror_storage_exports_the_local_live() {
    // Babel keeps the local as the storage and copies every write into the
    // property, including writes inside functions.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.inc = inc;
exports.mode = exports.count = void 0;
let count = exports.count = 0;
let mode = exports.mode = "off";
function inc() {
  exports.count = count = count + 1;
  if (count > 2) {
    exports.mode = mode = "on";
  }
  return count;
}
"#;
    let output = apply(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(output.contains("count = count + 1;"), "{output}");
    assert!(output.contains("mode = \"on\";"), "{output}");
    // Live exports of the locals, not snapshot copies.
    assert!(
        !output.contains("_count") && !output.contains("_mode"),
        "{output}"
    );
    assert_valid_esm(&output);
}

#[test]
fn mirror_storage_follows_a_reassigned_function() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.impl = void 0;
exports.swap = swap;
let impl = function () { return 1; };
exports.impl = impl;
function swap() {
  exports.impl = impl = function () { return 2; };
}
"#;
    let output = apply(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(!output.contains("_impl"), "{output}");
    assert_valid_esm(&output);
}

#[test]
fn mirror_storage_drops_update_mirrors() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.next = next;
exports.n = void 0;
let n = exports.n = 0;
function next() {
  var _n;
  _n = n++, exports.n = n, _n;
  exports.n = ++n;
  return n;
}
"#;
    let output = apply(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(output.contains("++n"), "{output}");
    assert_valid_esm(&output);
}

#[test]
fn mirror_storage_rewrites_reads_inside_function_declarations() {
    // Reads inside hoisted function bodies can run before the copy; the
    // mirror proves the property equals the local everywhere.
    let input = r#"
exports.helper = helper;
exports.run = run;
function helper() { return 1; }
function run() { return exports.helper() + exports.helper.length; }
"#;
    let output = apply(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(
        output.contains("return helper() + helper.length;"),
        "{output}"
    );
    assert_valid_esm(&output);
}

#[test]
fn mirror_storage_falls_back_to_property_storage_when_the_local_is_shadowed() {
    let input = r#"
let count = exports.count = 0;
function bump() { exports.count = count = count + 1; }
function read(count) { return exports.count + count; }
exports.bump = bump;
exports.read = read;
"#;
    let output = apply(input);
    assert!(!output.contains("exports"), "{output}");
    // The read must not resolve to the parameter.
    assert!(!output.contains("return count + count"), "{output}");
    assert_valid_esm(&output);
}

#[test]
fn mirror_storage_falls_back_to_property_storage_for_a_read_before_the_declaration() {
    // The property is `undefined` before the declaration runs; the lexical
    // local would throw in its temporal dead zone.
    let input = r#"
log(exports.value);
let value = exports.value = 1;
function bump() { exports.value = value = value + 1; }
exports.bump = bump;
"#;
    let output = apply(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(!output.contains("log(value)"), "{output}");
    assert_valid_esm(&output);
}

#[test]
fn mirror_storage_keeps_typescript_enum_initializers_for_un_enum() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.Mode = void 0;
var Mode;
(function (Mode) {
  Mode[Mode["On"] = 0] = "On";
  Mode[Mode["Off"] = 1] = "Off";
})(Mode || (exports.Mode = Mode = {}));
"#;
    let output = render_pipeline(input);
    assert!(output.contains("const Mode = {"), "{output}");
    assert!(!output.contains("exports"), "{output}");
}

/// Terser can inline the enum IIFE, which leaves the initializer outside any
/// call. `UnEnum` only folds the IIFE argument, so the inlined initializer
/// must be recovered as an ordinary mirror instead of being left to it.
#[test]
fn inlined_typescript_enum_initializers_are_recovered_as_mirrors() {
    // TypeScript 4.3.5, then Terser 5.51 with `toplevel` and two passes.
    let input = r#""use strict";var e;Object.defineProperty(exports,"__esModule",{value:!0}),exports.Kind=exports.Mode=void 0,(e=exports.Mode||(exports.Mode={})).ON="ON",e.OFF="OFF",(exports.Kind||(exports.Kind={})).A="A";"#;
    let output = render_pipeline(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(
        output.contains("Mode") && output.contains("Kind"),
        "{output}"
    );
    assert_valid_esm(&output);

    // The same initializer next to an export that the statement path owns.
    let input = r#""use strict";var e;Object.defineProperty(exports,"__esModule",{value:!0}),(e=exports.Mode||(exports.Mode={})).ON="ON",e.OFF="OFF",exports.Ready=1;"#;
    let output = render_pipeline(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(
        output.contains("Mode") && output.contains("Ready"),
        "{output}"
    );
    assert_valid_esm(&output);
}

#[test]
fn mirror_storage_recovers_alongside_a_babel_export_star_loop() {
    // The loop's `exports[key]` would fail the storage gate, but the loop
    // becomes `export * from` first; the aliased export is still mirrored.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
var _exportNames = { x: true, y: true, set: true };
exports.set = set;
exports.y = exports.x = void 0;
var _dep = require("./dep.js");
Object.keys(_dep).forEach(function (key) {
  if (key === "default" || key === "__esModule") return;
  if (Object.prototype.hasOwnProperty.call(_exportNames, key)) return;
  if (key in exports && exports[key] === _dep[key]) return;
  Object.defineProperty(exports, key, {
    enumerable: true,
    get: function () { return _dep[key]; }
  });
});
let x = exports.y = exports.x = 1;
function set() {
  exports.y = exports.x = x = 2;
}
"#;
    let output = apply(input);
    assert!(output.contains(r#"export * from "./dep.js";"#), "{output}");
    assert!(!output.contains("exports"), "{output}");
    // Both names export one live local; a later pass picks its name.
    assert!(
        output.contains("export let y = 1;") && output.contains("export { y as x };"),
        "{output}"
    );
    assert!(output.contains("y = 2;"), "{output}");
    let findings = validate_output_modules(&[
        ("entry.js".into(), output.clone()),
        ("dep.js".into(), "export const z = 1;".into()),
    ]);
    assert!(findings.is_empty(), "{findings:?}\n{output}");
}

#[test]
fn rollup_star_and_namespace_reexports_are_recovered() {
    // rollup shares `dep_js` between the namespace helper and the star loop.
    let inputs = [
        r#"
'use strict';

var dep_js = require('./dep.js');

function _interopNamespaceDefault(e) {
    var n = Object.create(null);
    if (e) {
        Object.keys(e).forEach(function (k) {
            if (k !== 'default') {
                var d = Object.getOwnPropertyDescriptor(e, k);
                Object.defineProperty(n, k, d.get ? d : {
                    enumerable: true,
                    get: function () { return e[k]; }
                });
            }
        });
    }
    n.default = e;
    return Object.freeze(n);
}

var dep_js__namespace = /*#__PURE__*/_interopNamespaceDefault(dep_js);

const own = 1;

exports.ns = dep_js__namespace;
exports.own = own;
Object.keys(dep_js).forEach(function (k) {
    if (k !== 'default' && !Object.prototype.hasOwnProperty.call(exports, k)) Object.defineProperty(exports, k, {
        enumerable: true,
        get: function () { return dep_js[k]; }
    });
});
"#,
        r#""use strict";var dep_js=require("./dep.js");function _interopNamespaceDefault(e){var t=Object.create(null);return e&&Object.keys(e).forEach(function(r){if("default"!==r){var n=Object.getOwnPropertyDescriptor(e,r);Object.defineProperty(t,r,n.get?n:{enumerable:!0,get:function(){return e[r]}})}}),t.default=e,Object.freeze(t)}var dep_js__namespace=_interopNamespaceDefault(dep_js);const own=1;exports.ns=dep_js__namespace,exports.own=1,Object.keys(dep_js).forEach(function(e){"default"===e||Object.prototype.hasOwnProperty.call(exports,e)||Object.defineProperty(exports,e,{enumerable:!0,get:function(){return dep_js[e]}})});"#,
    ];
    for input in inputs {
        let output = apply(input);
        for leftover in ["require", "exports", "_interopNamespaceDefault"] {
            assert!(!output.contains(leftover), "{leftover} left in {output}");
        }
        assert!(output.contains(r#"export * from "./dep.js";"#), "{output}");
        assert!(
            output.contains("import * as dep_js__namespace from"),
            "{output}"
        );
        assert!(output.contains("dep_js__namespace as ns"), "{output}");
    }
}
