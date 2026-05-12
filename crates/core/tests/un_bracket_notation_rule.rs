mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnBracketNotation;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnBracketNotation)
}

#[test]
fn transforms_bracket_notation_to_dot_or_numeric() {
    // Reused from packages/unminify/src/transformations/__tests__/un-bracket-notation.spec.ts
    let input = r#"
obj['bar'];
obj['bar'].baz;
obj['bar']['baz'];
obj['bar'].baz['qux'];

obj['1'];
obj['0'];
obj['00'];
obj['3.14'];
"#;
    let expected = r#"
obj.bar;
obj.bar.baz;
obj.bar.baz;
obj.bar.baz.qux;

obj[1];
obj[0];
obj['00'];
obj[3.14];
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_invalid_or_reserved_bracket_notation() {
    // Reused from packages/unminify/src/transformations/__tests__/un-bracket-notation.spec.ts
    let input = r#"
obj[a];
obj[''];
obj[' '];
obj['var'];
obj['let'];
obj['const'];
obj['await'];
obj['1var'];
obj['prop-with-dash'];
"#;
    let expected = r#"
obj[a];
obj[''];
obj[' '];
obj['var'];
obj['let'];
obj['const'];
obj['await'];
obj['1var'];
obj['prop-with-dash'];
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_computed_class_methods_and_fields() {
    let input = r#"
class C {
    ["value"]() {}
    get ["name"]() { return this.#n; }
    set ["name"](v) { this.#n = v; }
    ["count"] = 0;
    static ["create"]() {}
}
"#;
    let expected = r#"
class C {
    value() {}
    get name() { return this.#n; }
    set name(v) { this.#n = v; }
    count = 0;
    static create() {}
}
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_computed_object_literal_keys() {
    let input = r#"
const obj = {
    ["foo"]: 1,
    ["bar"]() {},
    get ["baz"]() { return 1; },
    set ["baz"](v) {},
    ["0"]: "zero",
};
"#;
    let expected = r#"
const obj = {
    foo: 1,
    bar() {},
    get baz() { return 1; },
    set baz(v) {},
    0: "zero",
};
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_invalid_computed_prop_names() {
    let input = r#"
class C {
    ["var"]() {}
    ["prop-with-dash"] = 1;
    [""]() {}
}
const obj = {
    ["let"]: 1,
    ["1var"]: 2,
};
"#;
    let expected = r#"
class C {
    ["var"]() {}
    ["prop-with-dash"] = 1;
    [""]() {}
}
const obj = {
    ["let"]: 1,
    ["1var"]: 2,
};
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_computed_constructor_method() {
    let input = r#"
class C {
    ["constructor"]() {
        return 1;
    }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn keeps_computed_proto_object_key() {
    let input = r#"
const obj = {
    ["__proto__"]: value,
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_bracket_in_assignment_targets() {
    let input = r##"
this["innerHTML"] = '';
this["#foo"]["innerHTML"] = '';
obj["bar"] = 1;
"##;
    let expected = r##"
this.innerHTML = '';
this["#foo"].innerHTML = '';
obj.bar = 1;
"##;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_string_literal_prop_names() {
    let input = r#"
const obj = {
    'readOnly': true,
    'id': 'foo',
    'createdCallback'() {},
    get 'value'() { return 1; },
    set 'value'(v) {},
};
"#;
    let expected = r#"
const obj = {
    readOnly: true,
    id: 'foo',
    createdCallback() {},
    get value() { return 1; },
    set value(v) {},
};
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_invalid_string_literal_prop_names() {
    let input = r#"
const obj = {
    'var': 1,
    'prop-with-dash': 2,
    '': 3,
};
"#;
    let expected = r#"
const obj = {
    'var': 1,
    'prop-with-dash': 2,
    '': 3,
};
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
