mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::{rules::UnJsx, RewriteLevel};

fn render_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |mark| UnJsx::new_with_level(mark, level))
}

#[test]
fn display_name_rename_updates_aliased_export_specifier() {
    // The public name is the alias `Z`, so the local binding stays renamable;
    // the specifier must follow the rename and keep the alias.
    let input = r#"
const c = styled.div;
c.displayName = "FancyCard";
export { c as Z };
use(c);
"#;
    let expected = r#"
const FancyCard = styled.div;
FancyCard.displayName = "FancyCard";
export { FancyCard as Z };
use(FancyCard);
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn display_name_rename_skips_exported_binding_but_renames_private_binding() {
    let input = r#"
export const a = styled.div;
a.displayName = "PublicCard";
const b = styled.div;
b.displayName = "PrivateCard";
use(a, b);
"#;
    let expected = r#"
export const a = styled.div;
a.displayName = "PublicCard";
const PrivateCard = styled.div;
PrivateCard.displayName = "PrivateCard";
use(a, PrivateCard);
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn lowercase_component_rename_skips_exported_binding_but_renames_private_binding() {
    let input = r#"
import { jsx } from "react/jsx-runtime";
export const widget = makeWidget();
const panel = makePanel();
const publicView = jsx(widget, {});
const privateView = jsx(panel, {});
"#;
    let expected = r#"
import { jsx } from "react/jsx-runtime";
export const widget = makeWidget();
const Panel = makePanel();
const publicView = jsx(widget, {});
const privateView = <Panel />;
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
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
fn converts_string_literal_hyphenated_attribute_name_to_valid_jsx() {
    let input = r#"
const icon = React.createElement("circle", {
  "data-app-bg-blur-radius": "12.5"
});
"#;
    let expected = r#"
const icon = <circle data-app-bg-blur-radius="12.5" />;
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn preserves_invalid_jsx_attribute_name_as_object_spread() {
    let input = r#"
const icon = React.createElement("circle", {
  "'data-app-bg-blur-radius'": "12.5"
});
"#;
    let expected = r#"
const icon = <circle {...{"'data-app-bg-blur-radius'": "12.5"}} />;
"#;

    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn keeps_create_element_used_as_member_object() {
    let input = r#"
const type = React.createElement(Component, null).type;
"#;

    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(output.contains(".type"), "member access was lost: {output}");
    assert_eq_normalized(&output, input);
}

#[test]
fn keeps_paren_wrapped_create_element_used_as_member_object() {
    // Parens cannot rescue `(<X/>).type`: the fixer strips redundant parens
    // later in the pipeline, degrading it to invalid `<X/>.type`.
    let input = r#"
const type = (React.createElement(Component, null)).type;
"#;

    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(
        !output.contains('<'),
        "paren-wrapped member object must not become JSX: {output}"
    );
}

#[test]
fn still_converts_arguments_of_preserved_member_object_call() {
    let input = r#"
const type = React.createElement(Outer, null, React.createElement(Inner, null)).type;
"#;

    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(
        output.contains("React.createElement(Outer"),
        "member-object root call must stay a call: {output}"
    );
    assert!(
        output.contains("<Inner"),
        "nested children must still convert: {output}"
    );
}

#[test]
fn keeps_create_element_in_optional_call_position() {
    // `value?.()` is an OptCall, not a CallExpr callee, so it needs its own
    // guard: `<X/>?.()` is invalid.
    let input = r#"
const a = React.createElement(Component, null)?.();
"#;

    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(
        !output.contains('<'),
        "optional-call position must not become JSX: {output}"
    );
}

#[test]
fn keeps_create_element_in_callee_new_and_tag_positions() {
    // A JSX element cannot stand as a callee, `new` callee, or template tag.
    let input = r#"
const a = React.createElement(Component, null)();
const b = new (React.createElement(Component, null))();
const c = React.createElement(Component, null)`text`;
"#;

    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(
        !output.contains('<'),
        "callee/new/tag positions must not become JSX: {output}"
    );
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
fn preserves_imported_name_when_capitalizing_component_binding() {
    let input = r#"
import { jsx } from "react/jsx-runtime";
import { widget } from "./dep";
const view = jsx(widget, {});
"#;
    let expected = r#"
import { jsx } from "react/jsx-runtime";
import { widget as Widget } from "./dep";
const view = <Widget />;
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
fn display_name_rename_keeps_leading_digit_name_valid() {
    let input = r#"
var t = () => React.createElement("svg", null);
t.displayName = "_4kSm";
use(t);
"#;
    let expected = r#"
var _4kSm = () => <svg />;
_4kSm.displayName = "_4kSm";
use(_4kSm);
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

#[test]
fn shadowed_object_assign_is_not_expanded_into_jsx_attrs() {
    // A local binding named `Object` is not the global: its `assign` may do
    // anything, so its arguments must not be flattened into separate JSX
    // attributes. The call survives as a single spread attribute instead.
    let input = r#"
import { jsx as _jsx } from "react/jsx-runtime";
function render(Object, props) {
    return _jsx("div", Object.assign({}, props));
}
"#;
    let expected = r#"
import { jsx as _jsx } from "react/jsx-runtime";
function render(Object, props) {
    return <div {...Object.assign({}, props)}/>;
}
"#;
    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn display_name_in_try_block_renames_the_enclosing_binding() {
    // Emitted by the displayName plugins: the assignment is wrapped in
    // `try`/`catch`, while the component binding lives in the enclosing scope.
    let input = r#"
var c = () => React.createElement("div", null);
try {
    c.displayName = "LoadableImage";
} catch (e) {}
var Baz = () => React.createElement(c, null);
"#;
    let expected = r#"
var LoadableImage = () => <div />;
try {
    LoadableImage.displayName = "LoadableImage";
} catch (e) {}
var Baz = () => <LoadableImage />;
"#;
    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn display_name_in_nested_block_renames_the_binding_declared_in_the_function() {
    let input = r#"
function o() {
    let e = i.createContext[s];
    if (!e) {
        Object.defineProperty(i.createContext, s, {
            value: e = i.createContext({}),
            configurable: true
        });
        e.displayName = "ApolloContext";
    }
    return e;
}
"#;
    let expected = r#"
function o() {
    let ApolloContext = i.createContext[s];
    if (!ApolloContext) {
        Object.defineProperty(i.createContext, s, {
            value: ApolloContext = i.createContext({}),
            configurable: true
        });
        ApolloContext.displayName = "ApolloContext";
    }
    return ApolloContext;
}
"#;
    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn hoisted_var_display_name_in_nested_block_renames_the_function_scope_binding() {
    let input = r#"
function F(x) {
    if (x) {
        var t = () => React.createElement("div", null);
        t.displayName = "Foo";
    }
    return React.createElement(t, null);
}
"#;
    let expected = r#"
function F(x) {
    if (x) {
        var Foo = () => <div />;
        Foo.displayName = "Foo";
    }
    return <Foo />;
}
"#;
    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn display_name_on_a_binding_declared_outside_the_processed_list_is_not_renamed() {
    // `t` is a parameter: the body's statement list does not declare it, so
    // renaming only the body would leave the parameter behind.
    let input = r#"
function F(t) {
    t.displayName = "Foo";
    return React.createElement(t, null);
}
"#;
    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(!output.contains("Foo."), "{output}");
    assert!(output.contains("function F(t)"), "{output}");
    assert!(output.contains("t.displayName = \"Foo\";"), "{output}");
}

#[test]
fn inline_component_alias_stays_inside_an_expression_bodied_arrow() {
    // The tag expression reads the arrow's parameter and must be evaluated on
    // every call; the alias belongs in the arrow's body, not before it.
    let input = r#"
const Icon = ({ type: t }) => React.createElement(pick(t), { className: "x" });
"#;
    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(!output.starts_with("const Component"), "{output}");
    assert!(output.contains("const Component = pick(t);"), "{output}");
    assert!(
        output.contains("return <Component className=\"x\"/>;"),
        "{output}"
    );
}

#[test]
fn inline_component_alias_is_not_hoisted_out_of_a_class_field_initializer() {
    // No statement list inside the initializer can hold the alias, and the
    // enclosing one runs in a different scope; leave the call as it is.
    let input = r#"
class Panel {
    icon = React.createElement(pick(this.kind), null);
}
"#;
    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(!output.contains("Component"), "{output}");
    assert!(
        output.contains("React.createElement(pick(this.kind), null)"),
        "{output}"
    );
}

#[test]
fn no_substitution_template_literal_tag_is_a_string_tag() {
    // `` createElement(`div`, …) `` names the intrinsic element the same way
    // `createElement("div", …)` does; it was aliased as `const Component = \`div\``
    // before.
    let input = r#"
function App() {
  return React.createElement(`div`, { className: "a" }, "hello");
}
"#;
    let expected = r#"
function App() {
  return <div className="a">hello</div>;
}
"#;
    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn const_template_literal_tag_is_inlined_like_a_string_const() {
    // Whatever the string-const path does to the binding afterwards, the
    // template spelling must come out the same.
    let template = r#"
const tag = `span`;
function App() {
  return React.createElement(tag, null, "hello");
}
"#;
    let string = template.replace("`span`", "\"span\"");
    let template_output = render_with_level(template, RewriteLevel::Standard);
    assert!(
        template_output.contains("<span>hello</span>"),
        "{template_output}"
    );
    assert_eq_normalized(
        &template_output.replace("`span`", "\"span\""),
        &render_with_level(&string, RewriteLevel::Standard),
    );
}

#[test]
fn template_literal_tag_keeps_the_string_capitalization_rule() {
    // A capitalized string tag names a component by string, which JSX cannot
    // express; the template spelling is rejected the same way.
    let input = r#"
function App() {
  return React.createElement(`Foo`, null, "hello");
}
"#;
    assert_eq_normalized(&render_with_level(input, RewriteLevel::Aggressive), input);
}

#[test]
fn template_literal_tag_with_substitution_is_not_a_string() {
    let input = r#"
function App(kind) {
  return React.createElement(`h${kind}`, null, "hello");
}
"#;
    let output = render_with_level(input, RewriteLevel::Standard);
    assert!(output.contains("`h${kind}`"), "{output}");
    assert!(!output.contains("<h"), "{output}");
}

#[test]
fn unrepresentable_string_tags_preserve_the_runtime_tag() {
    for tag in ["x.y", "x y", "x/y", "svg:", "svg:x:y"] {
        for literal in [format!("\"{tag}\""), format!("`{tag}`")] {
            let input = format!(
                "function App() {{ return React.createElement({literal}, null, \"hello\"); }}"
            );
            let expected = format!(
                "function App() {{ const Component = {literal}; return <Component>hello</Component>; }}"
            );
            assert_eq_normalized(&render_with_level(&input, RewriteLevel::Standard), &input);
            assert_eq_normalized(
                &render_with_level(&input, RewriteLevel::Aggressive),
                &expected,
            );
        }
    }
}

#[test]
fn valid_string_tags_keep_intrinsic_and_namespace_names() {
    for (literal, tag) in [
        ("`my-widget`", "my-widget"),
        ("`svg:path`", "svg:path"),
        (r#"`d\u0069v`"#, "div"),
    ] {
        let input = format!("function App() {{ return React.createElement({literal}, null); }}");
        let expected = format!("function App() {{ return <{tag} />; }}");
        assert_eq_normalized(
            &render_with_level(&input, RewriteLevel::Standard),
            &expected,
        );
    }
}

#[test]
fn unrepresentable_const_string_tag_keeps_its_runtime_value() {
    let input = "const tag = `x.y`; function App() { return React.createElement(tag, null); }";
    let expected = "const Tag = `x.y`; function App() { return <Tag />; }";
    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}

#[test]
fn lone_surrogate_strings_stay_string_literals() {
    // A lone surrogate has no UTF-8 form; JSX text built from it would
    // replace it with U+FFFD and change the rendered string.
    let input = r#"
const a = React.createElement("b", { title: "\uD83D" }, "\uD83D");
"#;
    let expected = r#"
const a = <b title={"\uD83D"}>{"\uD83D"}</b>;
"#;
    assert_eq_normalized(&render_with_level(input, RewriteLevel::Standard), expected);
}
