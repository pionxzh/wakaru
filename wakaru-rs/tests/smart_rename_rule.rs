mod common;

use common::{assert_eq_normalized, normalize, render_rule};
use wakaru_rs::rules::SmartRename;

fn apply(input: &str) -> String {
    render_rule(input, |_| SmartRename)
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
    assert!(output.contains("const x"), "should not rename when both obj and prop are short");
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
