mod common;

use common::{assert_eq_normalized, render_pipeline};

fn apply(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn transforms_indirect_call_to_direct_member_call() {
    // Reused pattern from packages/unminify/src/transformations/__tests__/un-indirect-call.spec.ts
    let input = r#"
import s from "react";

var countRef = (0, s.useRef)(0);
"#;
    // VarDeclToLetConst converts var to const since countRef is never reassigned.
    let expected = r#"
import s from "react";
const countRef = s.useRef(0);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_indirect_identifier_call() {
    let input = r#"
const result = (0, fn)(arg);
"#;
    let expected = r#"
const result = fn(arg);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_indirect_eval_call() {
    let input = r#"
const result = (0, eval)("this");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_object_wrapped_eval_call() {
    let input = r#"
const result = Object(eval)("this");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_multiple_indirect_calls() {
    // Reused pattern from packages/unminify/src/transformations/__tests__/un-indirect-call.spec.ts
    // UnEsm converts `const s = require("react")` → `import s from "react"`
    let input = r#"
const s = require("react");
var countRef = (0, s.useRef)(0);
var secondRef = (0, s.useMemo)(() => {}, []);
"#;
    // VarDeclToLetConst converts var to const since these vars are never reassigned.
    let expected = r#"
import s from "react";
const countRef = s.useRef(0);
const secondRef = s.useMemo(()=>{}, []);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_object_wrap_indirect_call() {
    // Object(fn.method)(args) → fn.method(args)
    // webpack bundles use Object() to avoid `this` binding on member expressions
    let input = r#"
Object(r.h)(e, "msg");
Object(r.validate)(x);
"#;
    let expected = r#"
r.h(e, "msg");
r.validate(x);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_object_wrap_call_through_a_local_object_binding() {
    // Only the global `Object` returns a function argument unchanged; a local
    // binding spelled `Object` can return anything.
    let input = r#"
function f(Object, r, e) {
    return Object(r.h)(e);
}
"#;
    let expected = r#"
function f(Object, r, e) {
    return Object(r.h)(e);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_indirect_call_of_local_method_that_reads_this() {
    // producer terser@5.51.2 { module: true, mangle: false } turns
    // `const m = o.method; return m();` into `(0, o.method)()`, which calls
    // the method with an undefined receiver.
    let input = r#"
const o = { method() { return this; } };
export function h() { return (0, o.method)(); }
"#;
    let expected = r#"
const o = {
    method() {
        return this;
    }
};
export function h() {
    return (0, o.method)();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_indirect_calls_of_assigned_and_static_methods_that_read_this() {
    let input = r#"
var a = {};
a.f = function () { return this; };
class C { static g() { return this; } }
var b = { k: function () { return this; } };
use((0, a.f)(), (0, C.g)(), Object(b.k)());
"#;
    let expected = r#"
const a = {};
a.f = function() {
    return this;
};
class C {
    static g() {
        return this;
    }
}
const b = {
    k() {
        return this;
    }
};
use((0, a.f)(), (0, C.g)(), Object(b.k)());
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn unwraps_indirect_call_of_local_member_that_ignores_this() {
    let input = r#"
const o = { method() { return 1; }, arrow: () => this, other: function () { return 2; } };
use((0, o.method)(), (0, o.arrow)(), (0, o.other)());
"#;
    let expected = r#"
const o = {
    method() {
        return 1;
    },
    arrow: ()=>this,
    other() {
        return 2;
    }
};
use(o.method(), o.arrow(), o.other());
"#;
    assert_eq_normalized(&apply(input), expected);
}
