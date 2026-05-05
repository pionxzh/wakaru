mod common;

use common::{assert_eq_normalized, normalize, render_rule};
use wakaru_rs::rules::SmartRename;

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
    assert!(output.contains("myVar"), "should not rename long names");
}

#[test]
fn member_init_rename_skips_both_short() {
    // Both obj and prop are short — combined name wouldn't help
    let input = r#"
const x = q.y;
"#;
    let output = apply(input);
    assert!(
        output.contains("const x"),
        "should not rename when both obj and prop are short"
    );
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
    assert!(
        !output.contains("const dRef = useRef();\nuse(d);"),
        "partial rename left stale use site:\n{output}"
    );
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
    assert!(
        output.contains("function inner(t)"),
        "shadowed parameter was renamed across scope:\n{output}"
    );
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
    assert!(
        output.contains("export const LContext = o.createContext(null);"),
        "top-level React rename was not applied:\n{output}"
    );
    assert!(
        output.contains("let l = arguments.length - 2;"),
        "shadowed local was incorrectly renamed:\n{output}"
    );
    assert!(
        output.contains("return l;"),
        "shadowed local use was incorrectly renamed:\n{output}"
    );
    assert!(
        output.contains("use(LContext);"),
        "outer use site was not renamed:\n{output}"
    );
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
    assert!(
        result.contains("SYMBOL_REACT_ELEMENT"),
        "should rename to SYMBOL_REACT_ELEMENT, got:\n{}",
        result
    );
    assert!(
        result.contains("SYMBOL_REACT_PORTAL"),
        "should rename to SYMBOL_REACT_PORTAL, got:\n{}",
        result
    );
    assert!(
        !result.contains("const ul "),
        "old name should be gone, got:\n{}",
        result
    );
}

#[test]
fn symbol_for_camel_case_key() {
    let input = r#"
const Ac = Symbol.for("react.forward_ref");
console.log(Ac);
"#;
    let result = apply(input);
    assert!(
        result.contains("SYMBOL_REACT_FORWARD_REF"),
        "should rename, got:\n{}",
        result
    );
}

#[test]
fn symbol_for_skips_long_names() {
    let input = r#"
const reactElement = Symbol.for("react.element");
console.log(reactElement);
"#;
    let result = apply(input);
    assert!(
        result.contains("reactElement"),
        "should keep long name as-is, got:\n{}",
        result
    );
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
    assert!(
        result.contains("SYMBOL_REACT_ELEMENT"),
        "should rename inside function scope, got:\n{}",
        result
    );
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
    assert!(
        result.contains("...rest"),
        "should rename ...d to ...rest, got:\n{}",
        result
    );
    assert!(
        result.contains("console.log(rest)"),
        "should rename reference, got:\n{}",
        result
    );
}

#[test]
fn rest_pattern_skips_long_names() {
    let input = r#"
const { foo, ...remaining } = obj;
console.log(remaining);
"#;
    let result = apply(input);
    assert!(
        result.contains("...remaining"),
        "should keep long name, got:\n{}",
        result
    );
}

#[test]
fn rest_pattern_avoids_collision() {
    let input = r#"
const rest = "taken";
const { foo, ...d } = obj;
console.log(rest, d);
"#;
    let result = apply(input);
    // `rest` is taken, so it should use `rest_1` or similar
    assert!(
        !result.contains("...d "),
        "should rename ...d, got:\n{}",
        result
    );
}

#[test]
fn rest_pattern_in_function_params() {
    let input = r#"
function foo({ name, ...r }) {
    return r;
}
"#;
    let result = apply(input);
    assert!(
        result.contains("...rest"),
        "should rename ...r to ...rest in params, got:\n{}",
        result
    );
}

#[test]
fn rest_pattern_does_not_collide_with_other_params() {
    // Arrow with existing `rest` param — renaming ...r to ...rest would be a duplicate
    let input = r#"
const fn2 = ({ name, ...r }, rest) => r;
"#;
    let result = apply(input);
    // Should not produce duplicate `rest` params
    assert!(
        !result.contains("...rest }, rest"),
        "must not create duplicate rest param:\n{}",
        result
    );
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
    assert!(
        output.contains("function e()"),
        "outer e must not be renamed:\n{}",
        output
    );
    assert!(
        output.contains("function t()"),
        "outer t must not be renamed:\n{}",
        output
    );
    assert!(
        output.contains("array: e"),
        "value e should stay:\n{}",
        output
    );
    assert!(
        output.contains("shape: t"),
        "value t should stay:\n{}",
        output
    );
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
    assert!(
        output.contains("import r from"),
        "should not rename when non-value use exists:\n{}",
        output
    );
}

#[test]
fn value_position_skips_long_names() {
    let input = r#"
import longName from "./m.js";
export default { Foo: longName };
"#;
    let output = apply(input);
    assert!(
        output.contains("import longName from"),
        "long names must not be renamed:\n{}",
        output
    );
}

#[test]
fn value_position_skips_when_target_is_existing_binding() {
    // `error` is already a binding — skip rather than emit `error_1`, which
    // would be strictly worse than the original `t`.
    let input = r#"
const error = "taken";
const f = (t) => ({ error: t });
use(error);
"#;
    let output = apply(input);
    assert!(
        output.contains("(t)"),
        "t should not be renamed when target binding exists:\n{}",
        output
    );
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
fn value_position_skips_when_target_would_shadow_use() {
    let input = r#"
const tW = makeSideCar();
function render(U) {
    const { sideCar } = U;
    return { sideCar: tW };
}
"#;
    let output = apply(input);
    assert!(
        output.contains("const tW = makeSideCar()"),
        "rename would be captured by inner sideCar binding:\n{}",
        output
    );
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
    assert!(
        output.contains("const s = 1"),
        "shared target must not be applied:\n{}",
        output
    );
    assert!(
        output.contains("const f = 2"),
        "shared target must not be applied:\n{}",
        output
    );
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
    assert!(
        output.contains("(payload)"),
        "should rename even when target is a property key elsewhere:\n{}",
        output
    );
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
    assert!(
        output.contains("const r = "),
        "should not rename when referenced by export:\n{}",
        output
    );
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
    assert!(
        output.contains("export let Jn = "),
        "should not rename exported declaration binding:\n{}",
        output
    );
    assert!(
        output.contains("version: Jn"),
        "should keep exported binding references explicit:\n{}",
        output
    );
}

#[test]
fn value_position_does_not_rename_to_reserved_keyword() {
    // Key `default` is reserved — should either skip or prefix with `_`.
    let input = r#"
import r from "./m.js";
export default { default: r };
"#;
    let output = apply(input);
    // Must not produce `import default from ...` (invalid).
    assert!(
        !output.contains("import default from"),
        "must not emit reserved keyword as binding name:\n{}",
        output
    );
}

#[test]
fn value_position_skips_computed_key() {
    let input = r#"
import r from "./m.js";
const k = "Foo";
export default { [k]: r };
"#;
    let output = apply(input);
    assert!(
        output.contains("import r from"),
        "must not rename via computed key:\n{}",
        output
    );
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
    assert!(
        output.contains("function inner(t)"),
        "inner parameter must not be renamed:\n{}",
        output
    );
    assert!(
        output.contains("const count = 1"),
        "outer binding must be renamed to `count`:\n{}",
        output
    );
}
