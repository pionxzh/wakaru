mod common;

use common::render;

#[test]
fn recovers_async_awaiter_alias_and_removes_decl() {
    let input = r#"
const V = this && this.__awaiter || ((a, b, c, d) => { return new Promise(() => {}); });
export function foo() {
    return V(this, undefined, undefined, function*() {
        const x = yield fetch("url");
        return x;
    });
}
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn recovers_generator_alias_and_removes_decl() {
    let input = r#"
const Z = this && this.__generator || ((a, b) => {
    return b.call(a, { label: 0, sent: function() {}, trys: [], ops: [] });
});
function foo() {
    return Z(this, function(state) {
        switch(state.label) {
            case 0:
                return [2, 42];
        }
    });
}
"#;
    let output = render(input);
    assert!(
        !output.contains("__generator"),
        "__generator alias should be removed: {output}"
    );
}

#[test]
fn does_not_touch_non_helper_patterns() {
    let input = r#"
const V = someOtherThing || fallback;
function foo() {
    return V(1, 2, 3);
}
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn non_async_ts_helper_is_left_for_its_consumer() {
    let input = r#"
const Y = this && this.__assign || function() { return Object.assign.apply(Object, arguments); };
function foo(Y) {
    let Y = "";
    return Y;
}
"#;
    let output = render(input);
    assert!(
        output.contains("const Y = this && this.__assign"),
        "__assign alias should be left for the object-spread consumer: {output}"
    );
    assert!(
        output.contains("function foo(Y)") && output.contains("let Y = \"\""),
        "shadowed locals should remain untouched: {output}"
    );
}

#[test]
fn handles_let_declaration() {
    let input = r#"
let Y = this && this.__assign || function() { return Object.assign.apply(Object, arguments); };
const x = Y({}, { a: 1 });
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_canonical_helpers_from_multi_declarator_decl() {
    let input = r#"
var __awaiter = this && this.__awaiter || function(thisArg, _arguments, P, generator) {
  return new (P || (P = Promise))(function(resolve) {
    resolve(generator.apply(thisArg, _arguments || []).next());
  });
}, __generator = this && this.__generator || function(thisArg, body) {
  return body.call(thisArg, { label: 0, sent: function() {}, trys: [], ops: [] });
};
function load_user(app_id) {
  return __awaiter(this, void 0, void 0, function() {
    var response;
    return __generator(this, function(_a) {
      switch (_a.label) {
        case 0:
          return [4, fetch_user(app_id)];
        case 1:
          response = _a.sent();
          return [2, response];
      }
    });
  });
}
"#;
    let output = render(input);
    assert!(
        !output.contains("this && this.__awaiter") && !output.contains("this && this.__generator"),
        "canonical helpers in a shared var declaration should be removed: {output}"
    );
    assert!(
        output.contains("async function load_user"),
        "async function should still be recovered after helper cleanup: {output}"
    );
}
