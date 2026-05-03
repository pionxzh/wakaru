mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::{rules::UnJsx, RewriteLevel};

fn render_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |mark| UnJsx::new_with_level(mark, level))
}

#[test]
fn converts_basic_create_element_to_jsx() {
    let input = r#"
function fn() {
  return React.createElement("div", {
    className: "flex flex-col",
    num: 1,
    foo: bar,
    onClick: function() {},
  });
}
"#;
    let expected = r#"
function fn() {
  return <div className="flex flex-col" num={1} foo={bar} onClick={function() {}} />;
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn converts_nested_children() {
    let input = r#"
function fn() {
  return React.createElement("div", null, child, React.createElement("span", null, "Hello"));
}
"#;
    let expected = r#"
function fn() {
  return <div>{child}<span>Hello</span></div>;
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn converts_automatic_runtime_children_and_key() {
    let input = r#"
const Foo = () => {
  return _jsxs("div", {
    children: [_jsx("p", {
      id: "a"
    }, void 0), _jsx("p", {
      children: "bar"
    }, "b"), _jsx("p", {
      children: "baz"
    }, c)]
  });
};
"#;
    let expected = r#"
const Foo = () => {
  return <div><p id="a" /><p key="b">bar</p><p key={c}>baz</p></div>;
};
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn standard_does_not_hoist_dynamic_component_tags() {
    let input = r#"
function fn() {
  return React.createElement(r ? "a" : "div", null, "Hello");
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), input);
}

#[test]
fn standard_hoists_dynamic_component_tags_with_strong_jsx_shape() {
    let input = r#"
function fn() {
  return _jsx(tt(), {
    className: "hero",
    children: "Hello"
  });
}
"#;
    let expected = r#"
function fn() {
  const Component = tt();
  return <Component className="hero">Hello</Component>;
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn aggressive_hoists_dynamic_component_tags() {
    let input = r#"
function fn() {
  return React.createElement(r ? "a" : "div", null, "Hello");
}
"#;
    let expected = r#"
function fn() {
  const Component = r ? "a" : "div";
  return <Component>Hello</Component>;
}
"#;

    assert_eq_normalized(
        &render_with_level(input, RewriteLevel::Aggressive),
        expected,
    );
}

#[test]
fn inlines_const_string_tag_names() {
    let input = r#"
function fn() {
  const Name = "div";
  return React.createElement(Name, null);
}
"#;
    let expected = r#"
function fn() {
  const Name = "div";
  return <div />;
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn renames_lowercase_component_bindings() {
    let input = r#"
function foo() {}
React.createElement(foo, null);
"#;
    let expected = r#"
function Foo() {}
<Foo />;
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn renames_components_from_display_name() {
    let input = r#"
var t = () => React.createElement("div", null);
t.displayName = "Foo-Bar";
var Baz = () => React.createElement("div", null, React.createElement(t, null));
"#;
    let expected = r#"
var FooBar = () => <div />;
FooBar.displayName = "Foo-Bar";
var Baz = () => <div><FooBar /></div>;
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn leaves_document_create_element_untouched() {
    let input = r#"
var x = document.createElement("div", attrs);
var y = window.document.createElement("div", attrs);
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), input);
}
