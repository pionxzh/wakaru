mod common;

use common::{assert_eq_normalized, normalize, render_rule};
use wakaru_core::rules::SmartRename;

fn apply(input: &str) -> String {
    render_rule(input, |_| SmartRename)
}

#[test]
fn jsx_component_alias_uses_source_name() {
    let input = r#"
function render(U) {
  const { sideCar } = U;
  const Tm = sideCar;
  return <Tm sideCar={medium} />;
}
"#;
    let expected = r#"
function render(U) {
  const { sideCar } = U;
  const SideCar = sideCar;
  return <SideCar sideCar={medium} />;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn jsx_component_alias_keeps_other_uses() {
    let input = r#"
function render(U) {
  const { sideCar } = U;
  const Tm = sideCar;
  use(Tm);
  return <Tm sideCar={medium} />;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn object_destructuring_rename_shorthand() {
    // { key: alias } where alias ≤2 chars → rename alias→key and convert to shorthand
    let input = r#"
const {
  gql: t,
  dispatchers: o,
  listener: i,
  sameName: sameName
} = n;
o.delete(t, i);
"#;
    let expected = r#"
const {
  gql,
  dispatchers,
  listener,
  sameName
} = n;
dispatchers.delete(gql, listener);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn object_destructuring_with_reserved_identifier() {
    let input = r#"
const {
  static: t,
  default: o,
} = n;
o.delete(t);
"#;
    let expected = r#"
const {
  static: _static,
  default: _default,
} = n;
_default.delete(_static);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn object_destructuring_in_function_parameter() {
    let input = r#"
function foo({
  gql: t,
  dispatchers: o,
  listener: i
}) {
  o.delete(t, i);
}
"#;
    let expected = r#"
function foo({
  gql,
  dispatchers,
  listener
}) {
  dispatchers.delete(gql, listener);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn object_destructuring_in_arrow_function_parameter() {
    let input = r#"
const foo2 = ({
  gql: t,
  dispatchers: o,
  listener: i
}) => {
  t[o].delete(i);
};
"#;
    let expected = r#"
const foo2 = ({
  gql,
  dispatchers,
  listener
}) => {
  gql[dispatchers].delete(listener);
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn react_rename_createcontext() {
    let input = r#"
const d = createContext(null);
const ef = o.createContext('light');
const ThemeContext = o.createContext('light');
"#;
    let expected = r#"
const DContext = createContext(null);
const EfContext = o.createContext('light');
const ThemeContext = o.createContext('light');
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn react_rename_usestate() {
    let input = r#"
const [e, f] = useState();
const [, g] = o.useState(0);
"#;
    let expected = r#"
const [e, setE] = useState();
const [, setG] = o.useState(0);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn react_rename_usereducer() {
    let input = r#"
const [e, f] = useReducer(r, i);
const [g, h] = o.useReducer(r, i, init);
"#;
    let expected = r#"
const [eState, fDispatch] = useReducer(r, i);
const [gState, hDispatch] = o.useReducer(r, i, init);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn react_rename_useref() {
    let input = r#"
const d = useRef();
const ef = o.useRef(null);
const buttonRef = o.useRef(null);
"#;
    let expected = r#"
const dRef = useRef();
const efRef = o.useRef(null);
const buttonRef = o.useRef(null);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn object_destructuring_in_function_body() {
    let input = r#"
function f() {
    let { line: z, col: Y } = pos;
    console.log(z, Y);
}
"#;
    let expected = r#"
function f() {
    let { line, col } = pos;
    console.log(line, col);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn object_destructuring_in_class_method_body() {
    let input = r#"
class Foo {
    bar() {
        let { task: _ } = result;
        return _.id;
    }
}
"#;
    let expected = r#"
class Foo {
    bar() {
        let { task } = result;
        return task.id;
    }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn member_init_rename_basic() {
    // var w = zw.NOT_APPLICABLE → rename w to zw_NOT_APPLICABLE
    let input = r#"
let w = zw.NOT_APPLICABLE;
console.log(w);
"#;
    let expected = r#"
let zw_NOT_APPLICABLE = zw.NOT_APPLICABLE;
console.log(zw_NOT_APPLICABLE);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn member_init_rename_short_obj() {
    // var z = q.length → rename z to q_length
    let input = r#"
const z = q.length;
while (z > 0) {}
"#;
    let expected = r#"
const q_length = q.length;
while (q_length > 0) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn member_init_rename_skips_long_names() {
    // Long variable names aren't minified, don't rename
    let input = r#"
const myVar = obj.prop;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn member_init_rename_skips_both_short() {
    // Both obj and prop are short — combined name wouldn't help
    let input = r#"
const x = q.y;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn member_init_rename_in_function_body() {
    let input = r#"
function f() {
    let _ = nodes.length;
    while (_ > 0) {
        _--;
    }
}
"#;
    let expected = r#"
function f() {
    let nodes_length = nodes.length;
    while (nodes_length > 0) {
        nodes_length--;
    }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// --- known-broken semantic regressions ---

#[test]
fn known_bug_react_rename_should_not_leave_stale_use_site() {
    let input = r#"
const d = useRef();
use(d);
"#;
    let output = normalize(&apply(input));
    insta::assert_snapshot!(output);
}

#[test]
fn known_bug_destructuring_rename_should_not_touch_shadowed_param() {
    let input = r#"
const { gql: t } = n;
function inner(t) {
  return t;
}
use(t);
"#;
    let output = normalize(&apply(input));
    insta::assert_snapshot!(output);
}

#[test]
fn react_rename_should_not_leak_into_shadowed_function_local() {
    let input = r#"
export const l = o.createContext(null);
function createElement() {
  let l = arguments.length - 2;
  if (e && e.defaultProps) {
    l = e.defaultProps;
  }
  return l;
}
use(l);
"#;
    let output = normalize(&apply(input));
    insta::assert_snapshot!(output);
}

// ============================================================
// Symbol.for renames
// ============================================================

#[test]
fn symbol_for_renamed_to_upper_snake_case() {
    let input = r#"
const ul = Symbol.for("react.element");
const At = Symbol.for("react.portal");
console.log(ul, At);
"#;
    let result = apply(input);
    insta::assert_snapshot!(result);
}

#[test]
fn symbol_for_camel_case_key() {
    let input = r#"
const Ac = Symbol.for("react.forward_ref");
console.log(Ac);
"#;
    let result = apply(input);
    insta::assert_snapshot!(result);
}

#[test]
fn symbol_for_skips_long_names() {
    let input = r#"
const reactElement = Symbol.for("react.element");
console.log(reactElement);
"#;
    let result = apply(input);
    assert_eq_normalized(&result, input);
}

#[test]
fn symbol_for_inside_function_scope() {
    let input = r#"
function init() {
    const ul = Symbol.for("react.element");
    return ul;
}
"#;
    let result = apply(input);
    insta::assert_snapshot!(result);
}

// ============================================================
// Rest pattern renames
// ============================================================

#[test]
fn rest_pattern_renamed_to_rest() {
    let input = r#"
const { foo, bar, ...d } = obj;
console.log(d);
"#;
    let result = apply(input);
    insta::assert_snapshot!(result);
}

#[test]
fn rest_pattern_skips_long_names() {
    let input = r#"
const { foo, ...remaining } = obj;
console.log(remaining);
"#;
    let result = apply(input);
    assert_eq_normalized(&result, input);
}

#[test]
fn rest_pattern_avoids_collision() {
    let input = r#"
const rest = "taken";
const { foo, ...d } = obj;
console.log(rest, d);
"#;
    let result = apply(input);
    // `rest` is taken, so ...d gets a suffixed name
    insta::assert_snapshot!(result);
}

#[test]
fn rest_pattern_in_function_params() {
    let input = r#"
function foo({ name, ...r }) {
    return r;
}
"#;
    let result = apply(input);
    insta::assert_snapshot!(result);
}

#[test]
fn rest_pattern_does_not_collide_with_other_params() {
    // Arrow with existing `rest` param — renaming ...r to ...rest would be a duplicate
    let input = r#"
const fn2 = ({ name, ...r }, rest) => r;
"#;
    let result = apply(input);
    insta::assert_snapshot!(result);
}

// ============================================================
// Value-position renames: short binding used only as `{ Key: x }`
// ============================================================

#[test]
fn value_position_rename_arrow_param_single_key() {
    // `t` used only as value of `error:` — rename to `error`.
    let input = r#"
const f = (e, t) => ({
    ...e,
    isLoading: false,
    error: t
});
"#;
    let expected = r#"
const f = (e, error) => ({
    ...e,
    isLoading: false,
    error
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn value_position_rename_default_import_used_once() {
    let input = r#"
import r from "./module-28.js";
export default {
    FrontPage: r
};
"#;
    let expected = r#"
import FrontPage from "./module-28.js";
export default {
    FrontPage
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn value_position_rename_multiple_params_each_unique_key() {
    let input = r#"
const f = (e, t) => ({
    data: e,
    error: t
});
"#;
    let expected = r#"
const f = (data, error) => ({
    data,
    error
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn value_position_skips_multiple_different_keys() {
    // `e` appears as value for many different keys — must NOT rename.
    let input = r#"
function outer() {
    function e() {}
    function t() {}
    const n = {
        array: e,
        bool: e,
        number: e,
        arrayOf: t,
        shape: t,
    };
    return n;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn value_position_skips_non_value_usage() {
    // `r` is used as a call callee / member access target — NOT only value position.
    let input = r#"
import r from "./m.js";
r();
const obj = { Foo: r };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn value_position_skips_long_names() {
    let input = r#"
import longName from "./m.js";
export default { Foo: longName };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn value_position_uses_suffix_when_target_is_existing_binding() {
    let input = r#"
const error = "taken";
const f = (t) => ({ error: t });
use(error);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn value_position_suffix_does_not_steal_natural_target() {
    // `t` wants `error` (blocked by top-level) → fallback `error_1`.
    // `n` wants `error_1` directly — the two-pass allocator must let `n`
    // claim `error_1` first, then give `t` the next available suffix.
    let input = r#"
const error = "taken";
const f = (t, n) => ({ error: t, error_1: n });
use(error);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn value_position_allows_target_name_in_unrelated_inner_scope() {
    let input = r#"
const tW = makeSideCar();
const obj = { sideCar: tW };
function render(U) {
    const { sideCar } = U;
    return sideCar;
}
"#;
    let expected = r#"
const sideCar = makeSideCar();
const obj = { sideCar };
function render(U) {
    const { sideCar } = U;
    return sideCar;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn value_position_uses_suffix_when_target_would_shadow_use() {
    let input = r#"
const tW = makeSideCar();
function render(U) {
    const { sideCar } = U;
    return { sideCar: tW };
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn value_position_skips_when_target_shared_by_multiple_bindings() {
    // Both `s` and `f` would want to rename to `$$typeof` — the target is
    // too generic to discriminate, so leave them alone.
    let input = r#"
const s = 1;
const f = 2;
use({ $$typeof: s });
use({ $$typeof: f });
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn value_position_allows_original_name_in_sibling_scope() {
    let input = r#"
function h(e, t) {
    const { initFoo } = t;
    return initFoo;
}
const connect = (e, t) => {
    const k = makeFoo(e, t);
    return setup({ initFoo: k });
};
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn value_position_allows_rename_when_key_is_only_property_name() {
    // `payload` appears only as a property key, not as a binding — still rename.
    let input = r#"
const handler = (e) => ({
    type: "X",
    payload: e
});
use(handler);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn value_position_skips_when_exported_by_name() {
    // `export { r }` is an other use — disqualifies.
    let input = r#"
const r = makeThing();
export { r };
const obj = { Foo: r };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn value_position_skips_exported_decl_binding() {
    // Renaming an exported declaration changes the public export name.
    let input = r#"
export let Jn = "7.50.0";
const metadata = {
    version: Jn
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn value_position_does_not_rename_to_reserved_keyword() {
    // Key `default` is reserved — should either skip or prefix with `_`.
    let input = r#"
import r from "./m.js";
export default { default: r };
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn value_position_skips_computed_key() {
    let input = r#"
import r from "./m.js";
const k = "Foo";
export default { [k]: r };
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn value_position_skips_shadowed_use() {
    // Outer `t` used once as value, but inner function has its own `t`.
    // The outer rename should still apply; inner `t` must stay.
    let input = r#"
const t = 1;
const obj = { count: t };
function inner(t) { return t + 1; }
use(obj, inner(2));
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn value_position_rename_from_jsx_attr() {
    let input = r#"
function render(c) {
  return <EContext.Provider value={c}>{children}</EContext.Provider>;
}
"#;
    let expected = r#"
function render(value) {
  return <EContext.Provider value={value}>{children}</EContext.Provider>;
}
"#;

    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn value_position_does_not_rename_from_invalid_jsx_attr_name() {
    let input = r#"
function render(U) {
  return <div data-state={U}>{children}</div>;
}
"#;

    assert_eq_normalized(&apply(input), input);
}

// ============================================================
// Sentry data-sentry-component renames
// ============================================================

#[test]
fn sentry_component_renames_function_decl() {
    let input = r#"
function a() {
  return <div data-sentry-component="MyComponent">Hello</div>;
}
"#;
    let expected = r#"
function MyComponent() {
  return <div data-sentry-component="MyComponent">Hello</div>;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sentry_component_renames_arrow_in_const() {
    let input = r#"
const a = () => <div data-sentry-component="MyComponent" />;
"#;
    let expected = r#"
const MyComponent = () => <div data-sentry-component="MyComponent" />;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sentry_component_renames_fn_expr_in_const() {
    let input = r#"
const a = function() {
  return <div data-sentry-component="MyComponent" />;
};
"#;
    let expected = r#"
const MyComponent = function() {
  return <div data-sentry-component="MyComponent" />;
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sentry_component_skips_already_named() {
    let input = r#"
function MyComponent() {
  return <div data-sentry-component="MyComponent">Hello</div>;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn sentry_component_skips_non_minified_name() {
    let input = r#"
function abc() {
  return <div data-sentry-component="MyComponent">Hello</div>;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn sentry_component_skips_conflict() {
    let input = r#"
const MyComponent = "taken";
function a() {
  return <div data-sentry-component="MyComponent">Hello</div>;
}
use(MyComponent);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn sentry_component_skips_duplicate_target() {
    let input = r#"
function a() {
  return <div data-sentry-component="Shared">Hello</div>;
}
function b() {
  return <span data-sentry-component="Shared">World</span>;
}
"#;
    let expected = r#"
function Shared() {
  return <div data-sentry-component="Shared">Hello</div>;
}
function b() {
  return <span data-sentry-component="Shared">World</span>;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sentry_component_camel_case_variant() {
    let input = r#"
const a = () => <div dataSentryComponent="NativeComponent" />;
"#;
    let expected = r#"
const NativeComponent = () => <div dataSentryComponent="NativeComponent" />;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sentry_component_nested_components() {
    let input = r#"
function a() {
  function b() {
    return <span data-sentry-component="Inner">nested</span>;
  }
  return <div data-sentry-component="Outer">{b()}</div>;
}
"#;
    let expected = r#"
function Outer() {
  function Inner() {
    return <span data-sentry-component="Inner">nested</span>;
  }
  return <div data-sentry-component="Outer">{Inner()}</div>;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sentry_component_export_default_named() {
    let input = r#"
export default function a() {
  return <div data-sentry-component="MyPage" />;
}
"#;
    let expected = r#"
export default function MyPage() {
  return <div data-sentry-component="MyPage" />;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sentry_component_skips_lowercase_name() {
    let input = r#"
function a() {
  return <div data-sentry-component="div">Hello</div>;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn sentry_component_skips_invalid_ident() {
    let input = r#"
function a() {
  return <div data-sentry-component="my-component">Hello</div>;
}
"#;
    assert_eq_normalized(&apply(input), input);
}
