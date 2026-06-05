mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::{rules::UnJsx, RewriteLevel};

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
fn minimal_does_not_convert_create_element_to_jsx() {
    let input = r#"
function fn() {
  return React.createElement("div", {
    className: "flex flex-col",
    children: "hello",
  });
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn removes_unused_imported_create_element_after_classic_jsx_conversion() {
    let input = r#"
import { Component, createElement } from "./react.js";

class App extends Component {
  render() {
    return createElement("div", null, "hello");
  }
}
"#;
    let expected = r#"
import { Component } from "./react.js";

class App extends Component {
  render() {
    return <div>hello</div>;
  }
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn keeps_imported_create_element_when_still_referenced() {
    let input = r#"
import { createElement } from "./react.js";

const el = createElement;
const view = createElement("div", null);
"#;
    let expected = r#"
import { createElement } from "./react.js";

const el = createElement;
const view = <div />;
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
fn standard_hoists_minifier_inlined_jsx_component_tags() {
    let input = r#"
function fn() {
  render(React.createElement(() => React.createElement(Fragment, null, child), null), mountNode);
}
"#;
    let expected = r#"
function fn() {
  const InlineComponent = () => <>{child}</>;
  render(<InlineComponent />, mountNode);
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn standard_does_not_hoist_inline_function_tags_without_jsx_body() {
    let input = r#"
function fn() {
  return React.createElement(() => value, null);
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), input);
}

#[test]
fn standard_does_not_hoist_inline_function_tags_with_only_nested_jsx() {
    let input = r#"
function fn() {
  return React.createElement(() => {
    const nested = () => <div />;
    return value;
  }, null);
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), input);
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
fn renames_lowercase_member_alias_component_from_property_name() {
    let input = r#"
function render(U) {
  const tm = U.sideCar;
  return React.createElement(tm, null);
}
"#;
    let expected = r#"
function render(U) {
  const SideCar = U.sideCar;
  return <SideCar />;
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn renames_lowercase_var_component_bindings() {
    let input = r#"
function Content(children) {
  return X.jsx(ea, {
    children
  });
}
var ea = styled.div();
"#;
    let expected = r#"
function Content(children) {
  return <Ea>{children}</Ea>;
}
var Ea = styled.div();
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn lowercase_var_component_rename_does_not_affect_dom_create_element() {
    let input = r#"
function render(doc, t) {
  var i = t.type;
  return doc.createElement(i, {
    is: t.is
  });
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), input);
}

#[test]
fn lowercase_var_component_rename_does_not_capture_prop_component() {
    let input = r#"
export function Icon({ icon, className }) {
  return J.jsx(icon, {
    className
  });
}
function Wrapper(props) {
  return J.jsx("svg", props);
}
"#;
    let expected = r#"
export function Icon({ icon, className }) {
  return J.jsx(icon, {
    className
  });
}
function Wrapper(props) {
  return <svg {...props}/>;
}
"#;

    let output = render_with_level(input, RewriteLevel::Standard);
    assert_eq_normalized(&output, expected);
}

#[test]
fn lowercase_var_component_rename_does_not_capture_shadowed_alias() {
    let input = r#"
function Icon(U) {
  var icon = U.icon;
  var wrapper = icon;
  return J.jsx(wrapper, {
    className: U.className
  });
}
function wrapper(props) {
  return J.jsx("svg", props);
}
"#;
    let expected = r#"
function Icon(U) {
  var icon = U.icon;
  var wrapper = icon;
  return J.jsx(wrapper, {
    className: U.className
  });
}
function wrapper(props) {
  return <svg {...props}/>;
}
"#;

    let output = render_with_level(input, RewriteLevel::Standard);
    assert_eq_normalized(&output, expected);
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

#[test]
fn converts_aliased_import_pragmas() {
    let input = r#"
import { jsx as t, jsxs as l } from "react/jsx-runtime";

function App() {
  return l("div", {
    children: [
      t("span", { children: "hello" }),
      t("span", { children: "world" })
    ]
  });
}
"#;
    let expected = r#"
import { jsx as t, jsxs as l } from "react/jsx-runtime";

function App() {
  return <div><span>hello</span><span>world</span></div>;
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn converts_aliased_dev_runtime_pragmas() {
    let input = r#"
import { jsxDEV as d } from "react/jsx-dev-runtime";

function App() {
  return d("div", { className: "app", children: "hello" });
}
"#;
    let expected = r#"
import { jsxDEV as d } from "react/jsx-dev-runtime";

function App() {
  return <div className="app">hello</div>;
}
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn removes_unused_classic_create_element_named_import() {
    let input = r#"
import { createElement } from "react";

export const app = createElement("div", null);
"#;
    let expected = r#"
import "react";

export const app = <div />;
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn keeps_classic_create_element_import_when_still_referenced() {
    let input = r#"
import { createElement } from "react";

const el = createElement("div", null);
export const factory = createElement;
"#;
    let expected = r#"
import { createElement } from "react";

const el = <div />;
export const factory = createElement;
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}
