mod common;

use common::{assert_eq_normalized, render_pipeline_until_with_level, render_rule};
use wakaru_core::{rules::ExtractInlinedFunction, RewriteLevel};

fn apply_rule(input: &str) -> String {
    render_rule(input, |_| {
        ExtractInlinedFunction::new(RewriteLevel::Aggressive)
    })
}

#[test]
fn extracts_iife_from_variable_initializer() {
    let input = r#"
const offset = ((toast, opts) => {
  const { reverseOrder = false, gutter = 8 } = opts || {};
  const index = toasts.findIndex((entry) => entry.id === toast.id);
  return index + gutter + (reverseOrder ? 1 : 0);
})(activeToast, options);
"#;
    let expected = r#"
const computeOffset = (toast, opts) => {
  const { reverseOrder = false, gutter = 8 } = opts || {};
  const index = toasts.findIndex((entry) => entry.id === toast.id);
  return index + gutter + (reverseOrder ? 1 : 0);
};
const offset = computeOffset(activeToast, options);
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn extracts_iife_inside_nested_block() {
    let input = r#"
const rendered = items.map((item) => {
  const style = ((position, offset) => {
    const isTop = position.includes("top");
    return {
      transform: `translateY(${offset * (isTop ? 1 : -1)}px)`,
      ...(isTop ? { top: 0 } : { bottom: 0 }),
    };
  })(item.position || defaultPosition, calculateOffset(item));
  return renderItem(item, style);
});
"#;
    let expected = r#"
const rendered = items.map((item) => {
  const computeStyle = (position, offset) => {
    const isTop = position.includes("top");
    return {
      transform: `translateY(${offset * (isTop ? 1 : -1)}px)`,
      ...isTop ? { top: 0 } : { bottom: 0 },
    };
  };
  const style = computeStyle(item.position || defaultPosition, calculateOffset(item));
  return renderItem(item, style);
});
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn derives_name_from_object_pattern_initializer() {
    let input = r#"
const { toasts, handlers } = ((toastOptions) => {
  const toasts = readToasts(toastOptions);
  return { toasts, handlers: createHandlers(toasts) };
})(options);
"#;
    let expected = r#"
const computeToasts = (toastOptions) => {
  const toasts = readToasts(toastOptions);
  return { toasts, handlers: createHandlers(toasts) };
};
const { toasts, handlers } = computeToasts(options);
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn suffixes_helper_name_on_conflict() {
    let input = r#"
const computeConfig = existing;
const config = ((input, defaults) => {
  return { ...defaults, ...input };
})(userConfig, defaultConfig);
"#;
    let expected = r#"
const computeConfig = existing;
const computeConfig_1 = (input, defaults) => {
  return { ...defaults, ...input };
};
const config = computeConfig_1(userConfig, defaultConfig);
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn suffixes_helper_name_to_avoid_shadowing_outer_reference() {
    let input = r#"
const computeStyle = readStyle;
function render(item) {
  const style = ((value) => {
    return computeStyle(value);
  })(item.value);
  return style;
}
"#;
    let expected = r#"
const computeStyle = readStyle;
function render(item) {
  const computeStyle_1 = (value) => {
    return computeStyle(value);
  };
  const style = computeStyle_1(item.value);
  return style;
}
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn standard_pipeline_keeps_iife() {
    let input = r#"
const offset = ((toast, opts) => {
  return toast.id + opts.gutter;
})(activeToast, options);
"#;
    let output =
        render_pipeline_until_with_level(input, "ExtractInlinedFunction", RewriteLevel::Standard);
    assert!(
        output.contains("((toast, opts)=>"),
        "standard mode should not extract inlined function:\n{output}"
    );
}

#[test]
fn aggressive_pipeline_extracts_iife() {
    let input = r#"
const offset = ((toast, opts) => {
  return toast.id + opts.gutter;
})(activeToast, options);
"#;
    let output =
        render_pipeline_until_with_level(input, "ExtractInlinedFunction", RewriteLevel::Aggressive);
    assert!(
        output.contains("const computeOffset ="),
        "aggressive mode should extract inlined function:\n{output}"
    );
    assert!(
        output.contains("const offset = computeOffset(activeToast, options);"),
        "aggressive mode should replace initializer call:\n{output}"
    );
}

#[test]
fn rejects_arguments_usage() {
    let input = r#"
const value = (function(item) {
  return arguments.length + item.id;
})(activeItem);
"#;
    let output = apply_rule(input);
    assert!(
        !output.contains("computeValue"),
        "arguments usage should not be extracted:\n{output}"
    );
    assert!(
        output.contains("arguments.length"),
        "arguments usage should be preserved:\n{output}"
    );
}

#[test]
fn rejects_when_containing_scope_has_direct_eval() {
    let input = r#"
function render(activeItem) {
  eval("computeValue");
  const value = ((item) => {
    return item.id;
  })(activeItem);
  return value;
}
"#;
    let output = apply_rule(input);
    assert!(
        !output.contains("computeValue ="),
        "direct eval in containing scope should block extraction:\n{output}"
    );
    assert!(
        output.contains("eval(\"computeValue\")"),
        "direct eval should be preserved:\n{output}"
    );
}

#[test]
fn rejects_when_candidate_body_has_direct_eval() {
    let input = r#"
const value = ((item) => {
  eval("item.id");
  return item.id;
})(activeItem);
"#;
    let output = apply_rule(input);
    assert!(
        !output.contains("computeValue ="),
        "direct eval inside candidate should block extraction:\n{output}"
    );
    assert!(
        output.contains("eval(\"item.id\")"),
        "direct eval should be preserved:\n{output}"
    );
}

#[test]
fn rejects_non_simple_params() {
    let input = r#"
const value = (({ item }) => {
  return item.id;
})(payload);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn rejects_multi_declarator_statement() {
    let input = r#"
const a = 1, value = ((item) => {
  return item.id;
})(activeItem);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}
