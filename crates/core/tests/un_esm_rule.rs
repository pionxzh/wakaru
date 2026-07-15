mod common;

use common::{
    assert_eq_normalized, render_pipeline, render_pipeline_until, render_pipeline_until_with_level,
};
use wakaru_core::RewriteLevel;

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
fn module_exports_default() {
    let input = "module.exports = 1;";
    let expected = "export default 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
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
fn exports_named_const() {
    let input = "exports.foo = 1;";
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
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
fn define_property_member_getter_rejects_binding_escape() {
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
    let expected = r#"
value;
mutate(exports);
export default exports.default;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_blocks_rebinding_exports() {
    let input = r#"
exports.default = value;
exports = other;
module.exports = exports.default;
"#;
    let expected = r#"
value;
exports = other;
export default exports.default;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
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
fn export_dedup_preserves_dropped_rhs_evaluation() {
    let input = r#"
exports.foo = sideEffect1();
exports.foo = sideEffect2();
"#;
    let expected = r#"
sideEffect1();
export const foo = sideEffect2();
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
fn void_only_export_removed() {
    let input = "exports.foo = void 0;";
    let expected = "";
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
consume(r.default, read(other));
"#;
    let expected = r#"
import r from "./dep.js";
let n;
function read(r) {
  return r.default;
}
consume(r, read(other));
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
