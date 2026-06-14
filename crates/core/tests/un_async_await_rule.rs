mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnAsyncAwait;

// ── __generator only ────────────────────────────────────────────────────────

fn apply(input: &str) -> String {
    let input = format!("{TS_HELPERS}\n{input}");
    render_rule(&input, |_| UnAsyncAwait)
}

fn apply_without_helpers(input: &str) -> String {
    render_rule(input, |_| UnAsyncAwait)
}

const TS_HELPERS: &str = r#"
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
  return new (P || (P = Promise))(function (resolve, reject) {
    function fulfilled(value) { step(generator.next(value)); }
    function rejected(value) { step(generator["throw"](value)); }
    function step(result) { result.done ? resolve(result.value) : Promise.resolve(result.value).then(fulfilled, rejected); }
    step((generator = generator.apply(thisArg, _arguments || [])).next());
  });
};
var __generator = (this && this.__generator) || function (thisArg, body) {
  var _ = { label: 0, sent: function() { return t[1]; }, trys: [], ops: [] }, f, y, t, g;
};
"#;

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
fn generator_yield_star_unwraps_values_helper() {
    let input = r#"
function read_all(source) {
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        return [4 /*yield*/, start_read(source)];
      case 1:
        _a.sent();
        return [5 /*yield**/, __values(read_chunks(source))];
      case 2:
        _a.sent();
        return [4 /*yield*/, finish_read(source)];
      case 3:
        return [2 /*return*/, _a.sent()];
    }
  });
}
"#;
    let expected = r#"
function* read_all(source) {
  yield start_read(source);
  yield* read_chunks(source);
  return yield finish_read(source);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_yield_star_unwraps_minified_values_helper() {
    // After minification the `__values` / `_ts_values` wrapper loses its name,
    // but the helper body shape (single iterable param, `Symbol.iterator`,
    // `TypeError`) is preserved. The delegate-yield opcode must still strip it
    // and the now-dead helper must be removed.
    let input = r#"
function v(o) {
  var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
  if (m) return m.call(o);
  if (o && typeof o.length === "number") return {
    next: function() {
      if (o && i >= o.length) o = void 0;
      return { value: o && o[i++], done: !o };
    }
  };
  throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
}
function read_all(source) {
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        return [4 /*yield*/, start_read(source)];
      case 1:
        _a.sent();
        return [5 /*yield**/, v(read_chunks(source))];
      case 2:
        _a.sent();
        return [4 /*yield*/, finish_read(source)];
      case 3:
        return [2 /*return*/, _a.sent()];
    }
  });
}
"#;
    let expected = r#"
function* read_all(source) {
  yield start_read(source);
  yield* read_chunks(source);
  return yield finish_read(source);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minified_ts_generator_function_decl_is_detected_by_shape() {
    let input = r#"
function e(thisArg, body) {
  var state = {
    label: 0,
    sent: function() {},
    trys: [],
    ops: []
  };
  return body.call(thisArg, state);
}
function read_items(items) {
  return e(this, function(_a) {
    switch (_a.label) {
      case 0:
        return [4, first_item(items)];
      case 1:
        _a.sent();
        return [4, second_item(items)];
      case 2:
        _a.sent();
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
    let output = apply_without_helpers(input);
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
fn generator_try_catch_recovers_catch_binding() {
    // TSC lowers `catch (error)` to a function-scoped temp assigned from
    // `_a.sent()` inside the catch state: `error_1 = _a.sent(); handle(error_1)`.
    // The decoder must fold that alias back into the catch binding instead of
    // emitting `error_1 = error; handle(error_1)`.
    let input = r#"
function fetch_items(source) {
  var error_1;
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        _a.trys.push([0, 3, , 4]);
        return [4 /*yield*/, start_fetch(source)];
      case 1:
        _a.sent();
        return [4 /*yield*/, finish_fetch(source)];
      case 2:
        _a.sent();
        return [3 /*break*/, 4];
      case 3:
        error_1 = _a.sent();
        handle(error_1);
        return [3 /*break*/, 4];
      case 4:
        return [2 /*return*/];
    }
  });
}
"#;
    let expected = r#"
function* fetch_items(source) {
  var error_1;
  try {
    yield start_fetch(source);
    yield finish_fetch(source);
  } catch (error) {
    handle(error);
  }
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

#[test]
fn canonical_awaiter_name_without_helper_shape_is_not_proof() {
    let input = r#"
function __awaiter(thisArg, args, P, generator) {
  log("custom");
  return generator();
}
function foo() {
  return __awaiter(this, void 0, void 0, function* () {
    yield work();
  });
}
"#;
    let output = apply_without_helpers(input);
    assert!(
        output.contains("return __awaiter(this, void 0, void 0, function*"),
        "custom canonical-name helper must not be treated as a proven TS helper, got:\n{output}"
    );
    assert!(
        output.contains("log(\"custom\")"),
        "custom helper body must be preserved, got:\n{output}"
    );
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
fn async_param_hint_renames_numbered_generated_alias() {
    let input = r#"
function runTask(ab1) {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [3 /*break*/, {
            details: {
              targetName: ab1
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
async function runTask(targetName) {}
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
fn async_conditional_await_assignment_from_jump_state() {
    let input = r#"
function load_user(config) {
  return __awaiter(this, void 0, void 0, function () {
    var source, _tmp;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          if (!(config == null)) return [3 /*break*/, 2];
          return [4 /*yield*/, load_config()];
        case 1:
          _tmp = _a.sent();
          return [3 /*break*/, 3];
        case 2:
          _tmp = config;
        case 3:
          source = _tmp;
          return [2 /*return*/, source];
      }
    });
  });
}
"#;
    let expected = r#"
async function load_user(config) {
  var source, _tmp;
  _tmp = !(config == null) ? config : await load_config();
  source = _tmp;
  return source;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_transform_preserves_non_matching_helper_calls_in_nested_callbacks() {
    let input = r#"
function load(items) {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [4 /*yield*/, fetch_items()];
        case 1:
          return [2 /*return*/, items.map(function (item) {
            return __generator(item, item.value);
          })];
      }
    });
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("async function load(items)"),
        "outer async wrapper should still be restored, got:\n{output}"
    );
    assert!(
        output.contains("await fetch_items()"),
        "state machine yield should still become await, got:\n{output}"
    );
    assert!(
        output.contains("return __generator(item, item.value);"),
        "non-matching helper call inside nested callback must be preserved, got:\n{output}"
    );
    assert!(
        !output.contains("function(item) {}"),
        "nested callback body must not be erased, got:\n{output}"
    );
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
    console.error(error, 2);
  } finally {
    console.log("finally");
  }
  console.log(3);
  await 7;
  try {
    console.log(4);
    await x;
  } catch (error) {
    console.error(error, 5);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_loop_try_catch_recovers_index_loop_jumps() {
    let input = r#"
function collect_enabled(items) {
  return __awaiter(this, void 0, void 0, function () {
    var output, index, item, _a, _b, error_1, _c, _d;
    return __generator(this, function (_e) {
      switch (_e.label) {
        case 0:
          output = [];
          index = 0;
          _e.label = 1;
        case 1:
          if (!(index < items.length)) return [3 /*break*/, 7];
          item = items[index];
          if (!item.enabled) {
            return [3 /*break*/, 6];
          }
          _e.label = 2;
        case 2:
          _e.trys.push([2, 4, , 6]);
          _b = (_a = output).push;
          return [4 /*yield*/, fetch_item(item.id)];
        case 3:
          _b.apply(_a, [_e.sent()]);
          return [3 /*break*/, 6];
        case 4:
          error_1 = _e.sent();
          _d = (_c = output).push;
          return [4 /*yield*/, recover_item(item, error_1)];
        case 5:
          _d.apply(_c, [_e.sent()]);
          return [3 /*break*/, 6];
        case 6:
          index++;
          return [3 /*break*/, 1];
        case 7:
          return [2 /*return*/, output];
      }
    });
  });
}
"#;
    let expected = r#"
async function collect_enabled(items) {
  var output, index, item, _a, _b, error_1, _c, _d;
  output = [];
  index = 0;
  for (; index < items.length; index++) {
    item = items[index];
    if (!item.enabled) {
      continue;
    }
    try {
      _b = (_a = output).push;
      _b.apply(_a, [await fetch_item(item.id)]);
    } catch (error) {
      _d = (_c = output).push;
      _d.apply(_c, [await recover_item(item, error)]);
    }
  }
  return output;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn awaiter_wrapping_double_yield_becomes_double_await() {
    let input = r#"
function func() {
  return __awaiter(this, void 0, void 0, function* () {
    yield yield 1;
  });
}
"#;
    let expected = r#"
async function func() {
  await await 1;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_simple_for_loop_via_ts_state_machine() {
    let input = r#"
function iter(items) {
  var i;
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        i = 0;
      case 1:
        if (!(i < items.length)) return [3 /*break*/, 4];
        return [4 /*yield*/, items[i]];
      case 2:
        _a.sent();
        i++;
        return [3 /*break*/, 1];
      case 3:
      case 4:
        return [2 /*return*/];
    }
  });
}
"#;
    let expected = r#"
function* iter(items) {
  var i;
  i = 0;
  for (; i < items.length; i++) {
    yield items[i];
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn awaiter_standalone_iife() {
    let input = r#"
__awaiter(this, void 0, void 0, function* () {
  yield setup();
  yield run();
});
"#;
    let expected = r#"
(async function() {
  await setup();
  await run();
})();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
