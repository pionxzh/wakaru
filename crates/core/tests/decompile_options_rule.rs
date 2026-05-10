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
    .expect("decompile should succeed");

    let expected = r#"
import unused from "./x.js";
function helper() { return 1; }
export const value = 2;
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn dead_code_elimination_remains_enabled_by_default() {
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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

    let expected = r#"const x = U?.name;"#;
    assert_eq_normalized(&output, expected);
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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

    let expected = r#"
const t = foo;
bar(t);
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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

    let expected = r#"((i, s, O) => s.createElement(O))(window, document, 'script');"#;
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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

    let expected = r#"
function fn() {
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
    .expect("decompile should succeed");

    let expected = r#"
function fn() {
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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

    let expected = r#"fn(...args);"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_disables_arguments_default_param_recovery() {
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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

    let expected = r#"Number(x);"#;
    assert_eq_normalized(&output, expected);
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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

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
    .expect("decompile should succeed");

    let expected = r#"for (const x of items) { console.log(x); }"#;
    assert_eq_normalized(&output, expected);
}
