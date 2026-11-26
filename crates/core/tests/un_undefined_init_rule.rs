mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::rules::UnUndefinedInit;

fn apply(input: &str) -> String {
    render_rule(input, UnUndefinedInit::new)
}

#[test]
fn let_undefined_becomes_let() {
    let input = r#"let x = undefined"#;
    let expected = r#""#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn let_void_0_becomes_let() {
    let input = r#"let x = void 0"#;
    let expected = r#""#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn var_undefined_becomes_var() {
    let input = r#"var x = undefined"#;
    let expected = r#""#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn used_undefined_init_becomes_bare_decl() {
    let input = r#"
let x = undefined;
use(x);
"#;
    let expected = r#"
let x;
use(x);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn eval_observable_undefined_init_becomes_bare_decl() {
    let input = r#"
function f() {
  let x = undefined;
  eval("x");
}
"#;
    let expected = r#"
function f() {
  let x;
  eval("x");
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn const_undefined_unchanged() {
    // const must have an initializer
    let input = r#"const x = undefined"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn let_with_value_unchanged() {
    let input = r#"let x = 42"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn let_multiple_decls_partial() {
    let input = r#"let x = undefined, y = 42"#;
    let expected = "let y = 42;";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_destructuring_undefined_init_preserved() {
    let input = r#"let {} = undefined"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn array_destructuring_undefined_init_preserved() {
    let input = r#"let [] = undefined"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_undefined_init_in_module_with_direct_eval() {
    // `eval(s)` can assign `x` before the declaration resets it.
    let input = r#"
eval(s);
var x = undefined;
use(x);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_let_undefined_init_when_eval_can_rebind_undefined() {
    let input = r#"
function f(s) {
    eval(s);
    let x = undefined;
    let y = void 0;
    return [x, y];
}
"#;
    let expected = r#"
function f(s) {
    eval(s);
    let x = undefined;
    let y;
    return [x, y];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_undefined_init_in_with_body() {
    // Inside the body the initializer writes `o.x` when `o` has `x`.
    let input = r#"
with (o) {
    var x = void 0;
    use(x);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn loop_body_var_keeps_undefined_init() {
    // `var x = void 0` resets `x` on every iteration; a bare `var x` does not.
    let input = r#"
for (var i = 0; i < 2; i++) {
  var x = void 0;
  use(x);
  x = i;
}
while (a) {
  if (b) {
    var y = undefined;
    use(y);
  }
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_in_function_inside_loop_drops_undefined_init() {
    let input = r#"
for (;;) {
  (function () {
    var x = void 0;
    use(x);
  })();
}
"#;
    let expected = r#"
for (;;) {
  (function () {
    var x;
    use(x);
  })();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn redeclared_var_keeps_undefined_init() {
    // The second declaration resets `value`; dropping it returns 1.
    let input = r#"
function f(flag) {
  var value = 1;
  if (flag) {
    var value = undefined;
  }
  return value;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_redeclaring_parameter_keeps_undefined_init() {
    let input = r#"
function f(value) {
  var value = void 0;
  return value;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_redeclaring_function_keeps_undefined_init() {
    // The function declaration is hoisted above the reset.
    let input = r#"
function f() {
  var g = void 0;
  function g() {}
  return g;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_written_before_declaration_keeps_undefined_init() {
    // `var` hoists, so `x = 5` writes the same binding the declaration resets.
    let input = r#"
function f() {
  x = 5;
  var x = undefined;
  return x;
}
function g(items) {
  [y] = items;
  var y = void 0;
  return y;
}
function h() {
  z++;
  var z = void 0;
  return z;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_written_by_earlier_declarator_keeps_undefined_init() {
    let input = r#"
function f() {
  var a = x = 5, x = void 0;
  return [a, x];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_written_in_for_in_head_before_declaration_keeps_undefined_init() {
    let input = r#"
function f(obj) {
  for (key in obj) {}
  var key = void 0;
  return key;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_written_in_nested_function_keeps_undefined_init() {
    // The closure can run before the declaration resets the binding.
    let input = r#"
function f() {
  var set = function () {
    y = 2;
  };
  set();
  var y = undefined;
  return y;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_written_in_later_function_declaration_keeps_undefined_init() {
    // A hoisted function declared later in the source can still run first,
    // and so can a closure it creates.
    let input = r#"
function f() {
  run();
  var y = void 0;
  return y;
  function run() {
    y = 2;
  }
}
function g() {
  run()();
  var z = void 0;
  return z;
  function run() {
    return () => {
      z = 2;
    };
  }
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn module_var_written_in_later_default_export_function_keeps_undefined_init() {
    let input = r#"
var state = void 0;
use(state);
export default function set(value) {
  state = value;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn var_written_in_later_closure_drops_undefined_init() {
    // A function expression or arrow is created after the declaration ran.
    let input = r#"
function f(cb) {
  var z = void 0;
  cb(() => {
    z = 1;
  });
  cb(function () {
    z = 2;
  });
  return z;
}
"#;
    let expected = r#"
function f(cb) {
  var z;
  cb(() => {
    z = 1;
  });
  cb(function () {
    z = 2;
  });
  return z;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn single_var_with_later_write_drops_undefined_init() {
    let input = r#"
function f(a) {
  var x = void 0;
  use(x);
  x = a;
  return x;
}
"#;
    let expected = r#"
function f(a) {
  var x;
  use(x);
  x = a;
  return x;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn shadowing_write_in_nested_function_drops_undefined_init() {
    // The nested function writes its own `x`, not the outer binding.
    let input = r#"
function f() {
  var x = void 0;
  use(x);
  return function () {
    var x = 1;
    x = 2;
    return x;
  };
}
"#;
    let expected = r#"
function f() {
  var x;
  use(x);
  return function () {
    var x = 1;
    x = 2;
    return x;
  };
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn var_reset_after_possible_write_survives_pipeline() {
    let input = r#"
function redeclared(flag) {
  var value = 1;
  if (flag) {
    var value = void 0;
  }
  return value;
}
function writtenBefore() {
  x = 5;
  var x = void 0;
  return x;
}
function closureWrites() {
  var set = function () {
    y = 2;
  };
  set();
  var y = void 0;
  return y;
}
function laterClosure(cb) {
  var z = void 0;
  cb(function () {
    z = 1;
  });
  return z;
}
function hoistedAfter() {
  var v = void 0;
  run();
  return v;
  function run() {
    v = 2;
  }
}
function plain(a) {
  var w = void 0;
  console.log(w);
  w = a;
  return w;
}
"#;
    let expected = r#"
function redeclared(flag) {
  var value = 1;
  if (flag) {
    var value = undefined;
  }
  return value;
}
function writtenBefore() {
  x = 5;
  var x = undefined;
  return x;
}
function closureWrites() {
  const set = () => {
    y = 2;
  };
  set();
  var y = undefined;
  return y;
}
function laterClosure(cb) {
  let z;
  cb(function () {
    z = 1;
  });
  return z;
}
function hoistedAfter() {
  let v;
  run();
  return v;
  function run() {
    v = 2;
  }
}
function plain(a) {
  let w;
  console.log(w);
  w = a;
  return w;
}
"#;
    assert_eq_normalized(&render_pipeline(input), expected);
}
