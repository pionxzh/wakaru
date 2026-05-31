mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnAsyncAwait;

// ── __generator only ────────────────────────────────────────────────────────

fn apply(input: &str) -> String {
    render_rule(input, |_| UnAsyncAwait)
}

#[test]
fn simple_generator_yields() {
    // Reused from packages/unminify/src/transformations/__tests__/un-async-await.spec.ts
    let input = r#"
function func() {
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0: return [4 /*yield*/, 1];
      case 1:
        _a.sent();
        return [4 /*yield*/, 2];
      case 2:
        _a.sent();
        return [4 /*yield*/, 3];
      case 3:
        _a.sent();
        return [2 /*return*/];
    }
  });
}
"#;
    let expected = r#"
function* func() {
  yield 1;
  yield 2;
  yield 3;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_with_assigned_sent_values() {
    // Generator where _a.sent() is assigned (result = _a.sent())
    // Note: var declarations belong in the outer function, not the state machine
    let input = r#"
function func() {
  var x, y;
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        return [4 /*yield*/, foo];
      case 1:
        x = _a.sent();
        return [4 /*yield*/, bar];
      case 2:
        y = _a.sent();
        return [2 /*return*/, y];
    }
  });
}
"#;
    let expected = r#"
function* func() {
  var x, y;
  x = yield foo;
  y = yield bar;
  return y;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn swc_ts_generator_helper() {
    let input = r#"
function _ts_generator(thisArg, body) {
  var t, _ = {
    label: 0,
    sent: function() { return t[1]; },
    trys: [],
    ops: []
  };
}
function read_items(items) {
  return _ts_generator(this, function(_state) {
    switch (_state.label) {
      case 0:
        return [4, first_item(items)];
      case 1:
        _state.sent();
        return [4, second_item(items)];
      case 2:
        _state.sent();
        return [2];
    }
  });
}
"#;
    let expected = r#"
function* read_items(items) {
  yield first_item(items);
  yield second_item(items);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// ── __awaiter only (inner is already function*) ──────────────────────────────

#[test]
fn awaiter_wrapping_generator_fn() {
    // __awaiter wrapping a function* — just lift the body and mark async
    let input = r#"
function func(x) {
  return __awaiter(this, void 0, void 0, function* () {
    yield 2;
    try {
      yield 1;
      console.log();
      yield x;
    } catch (e) {
      console.error();
    } finally {
      console.log("finally");
    }
    console.log();
    yield 7;
    try {
      console.log();
      yield x;
    } catch (e) {
      console.error(e);
    }
  });
}
"#;
    let expected = r#"
async function func(x) {
  await 2;
  try {
    await 1;
    console.log();
    await x;
  } catch (e) {
    console.error();
  } finally {
    console.log("finally");
  }
  console.log();
  await 7;
  try {
    console.log();
    await x;
  } catch (e) {
    console.error(e);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// ── __awaiter + __generator combined ────────────────────────────────────────

#[test]
fn empty_async_function() {
    // Simplest combined case: empty body
    let input = r#"
function f() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      return [2 /*return*/];
    });
  });
}
"#;
    let expected = r#"
async function f() {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn empty_async_function_keeps_param_hints_from_erased_state_machine() {
    let input = r#"
function runTask(A, B, C, D, E) {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [3 /*break*/, {
            details: {
              resourceName: A,
              payload: JSON.stringify(B),
              attemptCount: C,
              waitMs: D
            }
          }];
        case 1:
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function runTask(resourceName, B, attemptCount, waitMs, E) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_param_hint_does_not_rename_param_used_by_default() {
    let input = r#"
function runTask(A, B = A, C) {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [3 /*break*/, {
            details: {
              resourceName: A,
              attemptCount: C
            }
          }];
        case 1:
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function runTask(A, B = A, attemptCount) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_with_simple_awaits() {
    // __awaiter + __generator: simple sequential awaits, no try/catch
    let input = r#"
function func() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0: return [4 /*yield*/, 1];
        case 1:
          _a.sent();
          return [4 /*yield*/, 2];
        case 2:
          _a.sent();
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function func() {
  await 1;
  await 2;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_with_return_value() {
    // __awaiter + __generator with assigned sent values and explicit return
    let input = r#"
function func() {
  return __awaiter(this, void 0, void 0, function () {
    var x, y;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [4 /*yield*/, foo];
        case 1:
          x = _a.sent();
          return [4 /*yield*/, bar];
        case 2:
          y = _a.sent();
          return [2 /*return*/, y];
      }
    });
  });
}
"#;
    let expected = r#"
async function func() {
  var x, y;
  x = await foo;
  y = await bar;
  return y;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_with_yield_arg_consuming_previous_sent() {
    // Terser can fold TypeScript output so one yield argument consumes the
    // previous _a.sent() value: return [4, (response = _a.sent()).json()].
    let input = r#"
function load_user(app_id) {
  return __awaiter(this, void 0, void 0, function () {
    var response, data;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [4, fetch_user(app_id)];
        case 1:
          return [4, (response = _a.sent()).json()];
        case 2:
          return [2, data = _a.sent()];
      }
    });
  });
}
"#;
    let expected = r#"
async function load_user(app_id) {
  var response, data;
  response = await fetch_user(app_id);
  data = await response.json();
  return data;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_with_advanced_intermediate_awaits() {
    // Ported from the JS suite's advanced async/await fixture.
    let input = r#"
function func() {
  return __awaiter(this, void 0, void 0, function () {
    var result, json;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          console.log('Before sleep');
          return [4 /*yield*/, sleep(1000)];
        case 1:
          _a.sent();
          return [4 /*yield*/, fetch('')];
        case 2:
          result = _a.sent();
          return [4 /*yield*/, result.json()];
        case 3:
          json = _a.sent();
          return [2 /*return*/, json];
      }
    });
  });
}
"#;
    let expected = r#"
async function func() {
  var result, json;
  console.log('Before sleep');
  await sleep(1000);
  result = await fetch('');
  json = await result.json();
  return json;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_with_try_catch_finally() {
    // Full __awaiter + __generator with try/catch/finally regions
    let input = r#"
function func() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          _a.label = 1;
        case 1:
          _a.trys.push([1, 3, 4, 5]);
          return [4 /*yield*/, 1];
        case 2:
          _a.sent();
          return [3 /*break*/, 5];
        case 3:
          _a.sent();
          return [3 /*break*/, 5];
        case 4:
          return [7 /*endfinally*/];
        case 5:
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function func() {
  try {
    await 1;
  } catch (error) {}
  finally {}
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restore_complete_async_await_complex_try_regions() {
    // Ported from the JS suite's full async/await restoration fixture.
    let input = r#"
function func(x) {
  return __awaiter(this, void 0, void 0, function () {
    var e_1, e_2;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0: return [4 /*yield*/, 2];
        case 1:
          _a.sent();
          _a.label = 2;
        case 2:
          _a.trys.push([2, 5, 6, 7]);
          return [4 /*yield*/, 1];
        case 3:
          _a.sent();
          console.log(1);
          return [4 /*yield*/, x];
        case 4:
          _a.sent();
          return [3 /*break*/, 7];
        case 5:
          e_1 = _a.sent();
          console.error(e_1, 2);
          return [3 /*break*/, 7];
        case 6:
          console.log("finally");
          return [7 /*endfinally*/];
        case 7:
          console.log(3);
          return [4 /*yield*/, 7];
        case 8:
          _a.sent();
          _a.label = 9;
        case 9:
          _a.trys.push([9, 11, , 12]);
          console.log(4);
          return [4 /*yield*/, x];
        case 10:
          _a.sent();
          return [3 /*break*/, 12];
        case 11:
          e_2 = _a.sent();
          console.error(e_2, 5);
          return [3 /*break*/, 12];
        case 12: return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function func(x) {
  var e_1, e_2;
  await 2;
  try {
    await 1;
    console.log(1);
    await x;
  } catch (error) {
    e_1 = error;
    console.error(e_1, 2);
  } finally {
    console.log("finally");
  }
  console.log(3);
  await 7;
  try {
    console.log(4);
    await x;
  } catch (error) {
    e_2 = error;
    console.error(e_2, 5);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
