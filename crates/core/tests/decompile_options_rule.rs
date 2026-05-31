mod common;

use common::assert_eq_normalized;
use wakaru_core::{decompile, DecompileOptions, RewriteLevel};

#[test]
fn dead_code_elimination_can_be_disabled() {
    let input = r#"
import unused from "./x.js";
function helper() { return 1; }
export const value = 2;
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
import unused from "./x.js";
function helper() { return 1; }
export const value = 2;
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn dead_code_elimination_is_off_by_default() {
    let input = r#"
import unused from "./x.js";
function helper() { return 1; }
export const value = 2;
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("import unused"),
        "default should preserve unused imports: {output}"
    );
    assert!(
        output.contains("function helper"),
        "default should preserve unused functions: {output}"
    );
}

#[test]
fn dead_code_elimination_opt_in_removes_dead_code() {
    let input = r#"
import unused from "./x.js";
function helper() { return 1; }
export const value = 2;
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            dead_code_elimination: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
import "./x.js";
export const value = 2;
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn minimal_disables_loose_optional_chaining_recovery() {
    let input = r#"const x = U == null ? undefined : U.name;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input);
}

#[test]
fn standard_keeps_loose_optional_chaining_recovery_enabled() {
    let input = r#"const x = U == null ? undefined : U.name;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"const x = U?.name;"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_preserves_use_strict_directives() {
    let input = r#"
function foo(a, b, c) {
  "use strict";
  for (var value of arguments) {
    a = b;
    b = c;
    c = value;
  }
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("\"use strict\""),
        "minimal should preserve strict directives: {output}"
    );
}

#[test]
fn standard_strips_use_strict_directives() {
    let input = r#"
function foo() {
  "use strict";
  return 1;
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        !output.contains("\"use strict\""),
        "standard should keep existing cleanup behavior: {output}"
    );
}

#[test]
fn minimal_simplifies_safe_identifier_indirect_calls() {
    let input = r#"
const value = (0, fn)(arg);
const ref = (0, s.useRef)(0);
const result = (0, eval)("this");
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
const value = fn(arg);
const ref = (0, s.useRef)(0);
const result = (0, eval)("this");
"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_preserves_jsx_runtime_calls() {
    let input = r#"
import { jsx as _jsx } from "react/jsx-runtime";
export function view() {
  return _jsx("div", {
    children: "hi"
  });
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("jsx(\"div\""),
        "minimal output should keep executable JSX runtime calls: {output}"
    );
    assert!(
        !output.contains("<div"),
        "minimal output should not emit raw JSX syntax: {output}"
    );
}

#[test]
fn standard_simplifies_indirect_calls() {
    let input = r#"
const ref = (0, s.useRef)(0);
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
const ref = s.useRef(0);
"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_preserves_function_expressions() {
    let input = r#"
const fn = function() {
  return value;
};
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input.trim());
}

#[test]
fn standard_keeps_arrow_function_recovery() {
    let input = r#"
const fn = function() {
  return value;
};
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
const fn = () => value;
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn standard_keeps_babel_strict_optional_chaining_assignment_recovery() {
    let input =
        r#"const x = (_a = e.ownerDocument) === null || _a === void 0 ? void 0 : _a.defaultView;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"const x = e.ownerDocument?.defaultView;"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_enables_non_babel_strict_optional_chaining_assignment_recovery() {
    let input =
        r#"const x = (n = e.ownerDocument) === null || n === void 0 ? void 0 : n.defaultView;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Aggressive,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"const x = e.ownerDocument?.defaultView;"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_enables_loose_optional_chaining_assignment_recovery() {
    let input = r#"const x = (n = e.ownerDocument) == null ? undefined : n.defaultView;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Aggressive,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"const x = e.ownerDocument?.defaultView;"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_disables_smart_inline_temp_var_inlining() {
    let input = r#"
const t = foo;
bar(t);
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
const t = foo;
bar(t);
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn minimal_keeps_var_decl_to_let_const_recovery_safe() {
    let input = r#"
function readBeforeInit() {
    var value = read();
    var limit = 1;
    function read() { return limit; }
    return value;
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
function readBeforeInit() {
    const value = read();
    var limit = 1;
    function read() { return limit; }
    return value;
}
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn standard_keeps_smart_inline_temp_var_inlining() {
    let input = r#"
const t = foo;
bar(t);
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
bar(foo);
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn minimal_disables_iife_param_rewrites() {
    let input = r#"
((i, s, o) => {
  return s.createElement(o);
})(window, document, 'script');
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"((i, s, o) => s.createElement(o))(window, document, 'script');"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn standard_keeps_iife_param_rewrites() {
    let input = r#"
((i, s, o) => {
  return s.createElement(o);
})(window, document, 'script');
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
((window_1, document_1) => {
  const O = 'script';
  return document_1.createElement(O);
})(window, document);
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn standard_disables_dynamic_jsx_component_alias_synthesis() {
    let input = r#"
function fn() {
  return React.createElement(r ? "a" : "div", null, "Hello");
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input.trim());
}

#[test]
fn minimal_preserves_executable_jsx_runtime_calls() {
    let input = r#"
function fn() {
  return _jsx("div", {
    className: "hero",
    children: "Hello"
  });
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input.trim());
}

#[test]
fn standard_enables_dynamic_jsx_component_alias_synthesis_for_strong_runtime_shape() {
    let input = r#"
function fn() {
  return _jsx(tt(), {
    className: "hero",
    children: "Hello"
  });
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
function FnComponent() {
  const Component = tt();
  return <Component className="hero">Hello</Component>;
}
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn aggressive_enables_dynamic_jsx_component_alias_synthesis() {
    let input = r#"
function fn() {
  return React.createElement(r ? "a" : "div", null, "Hello");
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Aggressive,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
function FnComponent() {
  const Component = r ? "a" : "div";
  return <Component>Hello</Component>;
}
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn minimal_disables_argument_spread_recovery() {
    let input = r#"fn.apply(undefined, args);"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input);
}

#[test]
fn minimal_disables_array_concat_spread_recovery() {
    let input = r#"const x = [a].concat(b);"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input);
}

#[test]
fn standard_keeps_argument_spread_recovery() {
    let input = r#"fn.apply(undefined, args);"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"fn(...args);"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_disables_array_concat_spread_recovery_for_call_args() {
    let input = r#"const x = [this].concat(args);"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input);
}

#[test]
fn standard_keeps_array_concat_spread_recovery() {
    let input = r#"const x = [this].concat(args);"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"const x = [this, ...args];"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_keeps_var_decl_recovery_without_default_param_recovery() {
    let input = r#"
function foo(a) {
  var b = !(arguments.length > 1 && arguments[1] !== undefined) || arguments[1];
  return b;
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
function foo(a) {
  const b = !(arguments.length > 1 && arguments[1] !== undefined) || arguments[1];
  return b;
}
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn standard_keeps_arguments_default_param_recovery() {
    let input = r#"
function foo(a) {
  var b = !(arguments.length > 1 && arguments[1] !== undefined) || arguments[1];
  return b;
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
function foo(a, b = true) {
  return b;
}
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn minimal_disables_object_alias_default_param_recovery() {
    let input = r#"
function foo(options) {
  const opts = options === undefined ? {} : options;
  return opts.name;
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input.trim());
}

#[test]
fn standard_keeps_object_alias_default_param_recovery() {
    let input = r#"
function foo(options) {
  const opts = options === undefined ? {} : options;
  return opts.name;
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
function foo(options = {}) {
  return options.name;
}
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn minimal_disables_esm_reconstruction() {
    let input = r#"module.exports = 1;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input);
}

#[test]
fn standard_keeps_esm_reconstruction() {
    let input = r#"module.exports = 1;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"export default 1;"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_disables_type_constructor_recovery() {
    let input = r#"+x;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input);
}

#[test]
fn standard_keeps_type_constructor_recovery() {
    let input = r#"+x;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"Number(x);"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_disables_static_class_field_recovery() {
    let input = r#"
var User = (function () {
    function User() {}
    User.role = "admin";
    return User;
}());
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("User.role = \"admin\""),
        "minimal mode should preserve assignment semantics: {output}"
    );
    assert!(
        !output.contains("static role = \"admin\""),
        "static field recovery requires standard+: {output}"
    );
}

#[test]
fn standard_keeps_static_class_field_recovery() {
    let input = r#"
var User = (function () {
    function User() {}
    User.role = "admin";
    return User;
}());
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("static role = \"admin\""),
        "standard mode should recover static fields: {output}"
    );
}

#[test]
fn minimal_disables_instance_class_field_recovery() {
    let input = r#"
function _defineProperty(e, r, t) {
    if (r in e) {
        Object.defineProperty(e, r, { value: t, enumerable: true, configurable: true, writable: true });
    } else {
        e[r] = t;
    }
    return e;
}
class Foo {
    constructor() {
        _defineProperty(this, "value", 1);
    }
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("this[\"value\"] = 1"),
        "minimal mode should preserve constructor assignment semantics: {output}"
    );
    assert!(
        !output.contains("\n    value = 1"),
        "instance field recovery requires standard+: {output}"
    );
}

#[test]
fn standard_keeps_instance_class_field_recovery() {
    let input = r#"
function _defineProperty(e, r, t) {
    if (r in e) {
        Object.defineProperty(e, r, { value: t, enumerable: true, configurable: true, writable: true });
    } else {
        e[r] = t;
    }
    return e;
}
class Foo {
    constructor() {
        _defineProperty(this, "value", 1);
    }
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("\n    value = 1"),
        "standard mode should recover instance fields: {output}"
    );
    assert!(
        !output.contains("_defineProperty(this, \"value\", 1)"),
        "Babel helper call should be promoted in standard mode: {output}"
    );
}

#[test]
fn minimal_disables_define_property_instance_class_field_recovery() {
    let input = r#"
class Foo {
    constructor() {
        Object.defineProperty(this, "value", {
            enumerable: true,
            configurable: true,
            writable: true,
            value: 1
        });
    }
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("Object.defineProperty(this, \"value\""),
        "minimal mode should preserve descriptor assignment semantics: {output}"
    );
    assert!(
        !output.contains("\n    value = 1"),
        "defineProperty instance field recovery requires standard+: {output}"
    );
}

#[test]
fn standard_keeps_define_property_instance_class_field_recovery() {
    let input = r#"
class Foo {
    constructor() {
        Object.defineProperty(this, "value", {
            enumerable: true,
            configurable: true,
            writable: true,
            value: 1
        });
    }
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(
        output.contains("\n    value = 1"),
        "standard mode should recover descriptor instance field: {output}"
    );
    assert!(
        !output.contains("Object.defineProperty(this, \"value\""),
        "descriptor call should be promoted in standard mode: {output}"
    );
}

#[test]
fn minimal_disables_tsc_private_class_field_recovery() {
    let input = r#"
var __classPrivateFieldGet = function(receiver, state, kind, f) {
    return state.get(receiver);
};
var _Foo_x;
class Foo {
    constructor() {
        _Foo_x.set(this, 1);
    }
    getX() {
        return __classPrivateFieldGet(this, _Foo_x, "f");
    }
}
_Foo_x = new WeakMap();
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(!output.contains("#x"), "{output}");
    assert!(output.contains("_Foo_x.set(this, 1)"), "{output}");
    assert!(
        output.contains("__classPrivateFieldGet(this, _Foo_x, \"f\")"),
        "{output}"
    );
}

#[test]
fn standard_keeps_tsc_private_class_field_recovery() {
    let input = r#"
var __classPrivateFieldGet = function(receiver, state, kind, f) {
    return state.get(receiver);
};
var _Foo_x;
class Foo {
    constructor() {
        _Foo_x.set(this, 1);
    }
    getX() {
        return __classPrivateFieldGet(this, _Foo_x, "f");
    }
}
_Foo_x = new WeakMap();
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert!(output.contains("#x = 1"), "{output}");
    assert!(output.contains("return this.#x"), "{output}");
}

#[test]
fn minimal_disables_arg_rest_recovery() {
    let input = r#"
function foo() {
  return arguments[0];
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input.trim());
}

#[test]
fn standard_keeps_arg_rest_recovery() {
    let input = r#"
function foo() {
  return arguments[0];
}
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"
function foo(...args) {
  return args[0];
}
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn minimal_disables_for_of_recovery() {
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(x); }"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    assert_eq_normalized(&output, input);
}

#[test]
fn standard_keeps_for_of_recovery() {
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(x); }"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code;

    let expected = r#"for (const x of items) { console.log(x); }"#;
    assert_eq_normalized(&output, expected);
}
