mod common;

use common::{assert_eq_normalized, render};

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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
