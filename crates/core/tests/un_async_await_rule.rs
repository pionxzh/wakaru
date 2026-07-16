mod common;

use common::{assert_eq_normalized, render, render_pipeline_between_with_facts, render_rule};
use wakaru_core::facts::{
    ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact, TypeScriptHelperKind,
};
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

fn cross_module_async_facts() -> ModuleFactsMap {
    cross_module_async_facts_with_factory(true)
}

fn cross_module_async_facts_with_factory(include_factory: bool) -> ModuleFactsMap {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            ts_helper_exports: vec![
                TypeScriptHelperExportFact {
                    exported: "__awaiter".into(),
                    local: Some("__awaiter".into()),
                    kind: TypeScriptHelperKind::Awaiter,
                },
                TypeScriptHelperExportFact {
                    exported: "__generator".into(),
                    local: Some("__generator".into()),
                    kind: TypeScriptHelperKind::Generator,
                },
            ],
            ts_helper_namespace_factory_exports: if include_factory {
                vec!["helperFactory".into()]
            } else {
                Vec::new()
            },
            ..Default::default()
        },
    );
    facts
}

fn apply_cross_module_facts(input: &str, facts: &ModuleFactsMap) -> String {
    render_pipeline_between_with_facts(input, "UnAsyncAwait", "UnAsyncAwait", facts, None)
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
fn cross_module_namespace_async_helpers_require_proven_facts() {
    let input = r#"
import * as helpers from "./helpers.js";
function func() {
  return helpers.__awaiter(this, void 0, void 0, function () {
    return helpers.__generator(this, function (_a) {
      switch (_a.label) {
        case 0: return [4, first()];
        case 1:
          _a.sent();
          return [4, second()];
        case 2:
          _a.sent();
          return [2];
      }
    });
  });
}
"#;
    let expected = r#"
import * as helpers from "./helpers.js";
async function func() {
  await first();
  await second();
}
"#;

    let facts = cross_module_async_facts();
    assert_eq_normalized(&apply_cross_module_facts(input, &facts), expected);

    let no_facts = ModuleFactsMap::new();
    assert_eq_normalized(&apply_cross_module_facts(input, &no_facts), input);
}

#[test]
fn cross_module_named_async_helper_aliases_use_export_facts() {
    let input = r#"
import { __awaiter as runAsync, __generator as runGenerator } from "./helpers.js";
function func() {
  return runAsync(this, void 0, void 0, function () {
    return runGenerator(this, function (_a) {
      switch (_a.label) {
        case 0: return [4, value()];
        case 1: return [2, _a.sent()];
      }
    });
  });
}
"#;
    let expected = r#"
import { __awaiter as runAsync, __generator as runGenerator } from "./helpers.js";
async function func() {
  return await value();
}
"#;

    assert_eq_normalized(
        &apply_cross_module_facts(input, &cross_module_async_facts()),
        expected,
    );
}

#[test]
fn cross_module_async_namespace_factory_requires_factory_fact() {
    let input = r#"
import { helperFactory } from "./helpers.js";
const helpers = helperFactory();
function func() {
  return helpers.__awaiter(this, void 0, void 0, function () {
    return helpers.__generator(this, function (_a) {
      switch (_a.label) {
        case 0: return [4, value()];
        case 1: return [2, _a.sent()];
      }
    });
  });
}
"#;
    let expected = r#"
import { helperFactory } from "./helpers.js";
const helpers = helperFactory();
async function func() {
  return await value();
}
"#;

    let facts = cross_module_async_facts();
    assert_eq_normalized(&apply_cross_module_facts(input, &facts), expected);

    let missing_factory = cross_module_async_facts_with_factory(false);
    assert_eq_normalized(&apply_cross_module_facts(input, &missing_factory), input);
}

#[test]
fn cross_module_async_namespace_factory_rejects_calls_with_arguments() {
    let input = r#"
import { helperFactory } from "./helpers.js";
const helpers = helperFactory(options);
function func() {
  return helpers.__awaiter(this, void 0, void 0, function* () {
    yield value();
  });
}
"#;
    assert_eq_normalized(
        &apply_cross_module_facts(input, &cross_module_async_facts()),
        input,
    );
}

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
fn async_transform_rolls_back_when_generator_state_machine_is_unsupported() {
    let input = r#"
function load() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [9, work()];
      }
    });
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return __awaiter(this, void 0, void 0, function()"),
        "failed generator decoding must preserve the awaiter wrapper, got:\n{output}"
    );
    assert!(
        !output.contains("async function load"),
        "an unresolved generator wrapper must not be marked async, got:\n{output}"
    );
}

#[test]
fn nested_unsupported_generator_does_not_roll_back_outer_async_transform() {
    let input = r#"
function load() {
  return __awaiter(this, void 0, void 0, function* () {
    function nested() {
      return __generator(this, function (_a) {
        switch (_a.label) {
          case 0:
            return [9, work()];
        }
      });
    }
    yield fetch_items();
    return nested;
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("async function load()"),
        "an independent nested wrapper must not block outer recovery, got:\n{output}"
    );
    assert!(
        output.contains("await fetch_items();"),
        "the outer generator yield should become await, got:\n{output}"
    );
    assert!(
        output.contains("return __generator(this, function(_a)"),
        "the unsupported nested wrapper must remain intact, got:\n{output}"
    );
}

// ── conditional forward jumps to a mid-machine join ─────────────────────────

#[test]
fn recovers_guarded_await_with_mid_machine_join() {
    // tsc emits this for `if (cond) { await ... }` without an else: the guard
    // jumps forward to a join label that still has statements after it.
    let input = r#"
function init() {
  return __awaiter(this, void 0, void 0, function () {
    var id;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          if (id = cache.get()) return [3 /*break*/, 2];
          return [4 /*yield*/, storage.getItemAsync(KEY)];
        case 1:
          id = (id = _a.sent()) != null ? id : make();
          _a.label = 2;
        case 2:
          storage.setItemAsync(KEY, id);
          return [2 /*return*/, id];
      }
    });
  });
}
"#;
    let expected = r#"
async function init() {
  var id;
  if (!(id = cache.get())) {
    id = (id = await storage.getItemAsync(KEY)) != null ? id : make();
  }
  storage.setItemAsync(KEY, id);
  return id;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn recovers_nested_guarded_awaits_with_mid_machine_joins() {
    let input = r#"
function init() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          if (ready) return [3 /*break*/, 4];
          return [4 /*yield*/, connect()];
        case 1:
          _a.sent();
          if (cached) return [3 /*break*/, 3];
          return [4 /*yield*/, warm_up()];
        case 2:
          _a.sent();
          _a.label = 3;
        case 3:
          mark_started();
          _a.label = 4;
        case 4:
          finish();
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function init() {
  if (!ready) {
    await connect();
    if (!cached) {
      await warm_up();
    }
    mark_started();
  }
  finish();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn mid_machine_join_rolls_back_when_guard_crosses_try_region() {
    // The guarded region (1..3) straddles the try-region boundary at label 2:
    // folding it into an `if` would silently move statements out of the
    // reconstructed try/catch. The wrapper must be preserved instead.
    let input = r#"
function init() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          if (ready) return [3 /*break*/, 3];
          prepare();
          _a.label = 1;
        case 1:
          _a.trys.push([1, 4, , 5]);
          risky();
          _a.label = 2;
        case 2:
          commit();
          _a.label = 3;
        case 3:
          return [4 /*yield*/, finish()];
        case 4:
          _a.sent();
          return [3 /*break*/, 5];
        case 5:
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return __awaiter(this, void 0, void 0, function()"),
        "a guard crossing a try-region boundary must roll back, got:\n{output}"
    );
    assert!(
        !output.contains("async function init"),
        "a guard crossing a try-region boundary must not be marked async, got:\n{output}"
    );
}

#[test]
fn mid_machine_join_rolls_back_when_region_contains_unresolved_jump() {
    // The guarded region contains its own jump into the machine, which the
    // fold cannot represent; recovery must fail closed and keep the wrapper.
    let input = r#"
function init() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          if (ready) return [3 /*break*/, 3];
          return [4 /*yield*/, connect()];
        case 1:
          _a.sent();
          if (skip) return [3 /*break*/, 1];
          warm_up();
          _a.label = 3;
        case 3:
          finish();
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return __awaiter(this, void 0, void 0, function()"),
        "an unresolvable jump inside the guarded region must roll back, got:\n{output}"
    );
    assert!(
        !output.contains("async function init"),
        "an unresolvable jump inside the guarded region must not be marked async, got:\n{output}"
    );
}

// ── awaiter thisArg handling ────────────────────────────────────────────────

#[test]
fn awaiter_with_captured_this_alias_rewrites_body_this() {
    // tsc captures the enclosing `this` (`var _this = this`) when the awaiter
    // sits inside a plain callback. Splicing the body into that callback
    // rebinds `this`, so references must be rewritten to the alias.
    let input = r#"
function make() {
  var self = this;
  return function () {
    return __awaiter(self, void 0, void 0, function* () {
      yield ready();
      return this.items.map(() => this.tag);
    });
  };
}
"#;
    let expected = r#"
function make() {
  var self = this;
  return async function() {
    await ready();
    return self.items.map(() => self.tag);
  };
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn awaiter_alias_rewrite_preserves_nested_function_this() {
    let input = r#"
function make() {
  var self = this;
  return function () {
    return __awaiter(self, void 0, void 0, function* () {
      yield ready();
      return function () { return this.own; };
    });
  };
}
"#;
    let expected = r#"
function make() {
  var self = this;
  return async function() {
    await ready();
    return function() {
      return this.own;
    };
  };
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn awaiter_with_expression_this_arg_is_preserved() {
    // A non-identifier thisArg would need re-evaluating per `this` reference;
    // the wrapper must be preserved instead.
    let input = r#"
function load(ctx) {
  return __awaiter(ctx.scope, void 0, void 0, function* () {
    yield ready();
    return this.items;
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return __awaiter(ctx.scope, void 0, void 0, function*()"),
        "an expression thisArg must preserve the awaiter wrapper, got:\n{output}"
    );
    assert!(
        !output.contains("async function load"),
        "an expression thisArg must not be marked async, got:\n{output}"
    );
}

#[test]
fn awaiter_alias_shadowed_inside_body_is_preserved() {
    let input = r#"
function make() {
  var self = this;
  return function () {
    return __awaiter(self, void 0, void 0, function* () {
      var self = other();
      yield ready();
      return this.items;
    });
  };
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return __awaiter(self, void 0, void 0, function*()"),
        "a shadowed alias must preserve the awaiter wrapper, got:\n{output}"
    );
}

#[test]
fn awaiter_iife_with_captured_this_alias_rewrites_body_this() {
    let input = r#"
var self = this;
__awaiter(self, void 0, void 0, function* () {
  yield tick();
  self_report(this.count);
});
"#;
    let output = apply(input);
    assert!(
        output.contains("(async function()"),
        "alias-thisArg awaiter IIFE should still unwrap, got:\n{output}"
    );
    assert!(
        output.contains("self_report(self.count);"),
        "IIFE body `this` must be rewritten to the alias, got:\n{output}"
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
      output.push(await fetch_item(item.id));
    } catch (error) {
      output.push(await recover_item(item, error));
    }
  }
  return output;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_memoized_method_apply_keeps_side_effecting_direct_receiver() {
    let input = r#"
function collect() {
  return __awaiter(this, void 0, void 0, function () {
    var _a;
    return __generator(this, function (_b) {
      switch (_b.label) {
        case 0:
          _a = get_output().push;
          return [4 /*yield*/, fetch_item()];
        case 1:
          _a.apply(get_output(), [_b.sent()]);
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function collect() {
  var _a;
  _a = get_output().push;
  _a.apply(get_output(), [await fetch_item()]);
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

#[test]
fn nested_arrow_generator_does_not_block_standalone_awaiter_iife() {
    let input = r#"
__awaiter(this, void 0, void 0, function* () {
  const nested = () => __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        return [9, work()];
    }
  });
  yield setup();
  return nested;
});
"#;
    let output = apply(input);
    assert!(
        output.contains("(async function()"),
        "an independent nested arrow must not block IIFE recovery, got:\n{output}"
    );
    assert!(
        output.contains("await setup();"),
        "the standalone awaiter yield should become await, got:\n{output}"
    );
    assert!(
        output.contains("=>__generator(this, function(_a)"),
        "the unsupported nested arrow wrapper must remain intact, got:\n{output}"
    );
}

// ── Terser-compressed state machines ────────────────────────────────────────

#[test]
fn terser_compressed_generator_loop_with_ternary_return() {
    // Terser compresses `case 1: if (!(cond)) return [3,4]; return [4,X];`
    // into `case 1: return cond ? [4,X] : [3,4];`
    // and `case 3: i++; return [3,1]` into `case 3: return i++,[3,1]`
    let input = r#"
function iter_items(items) {
  var index;
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        index = 0, _a.label = 1;
      case 1:
        return index < items.length ? [4, items[index]] : [3, 4];
      case 2:
        _a.sent(), _a.label = 3;
      case 3:
        return index++, [3, 1];
      case 4:
        return [2];
    }
  });
}
"#;
    let expected = r#"
function* iter_items(items) {
  var index;
  index = 0;
  for (; index < items.length; index++) {
    yield items[index];
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn terser_compressed_async_loop_with_comma_sequence_in_ternary() {
    // Terser compresses the awaiter+generator loop where `results.push(…)` is
    // split into method-caching setup before the yield:
    //   case 1: return cond ? (_b=(_a=results).push, [4, transform_item(…)]) : [3,4]
    //   case 2: _b.apply(_a, [_c.sent()]), _c.label = 3
    //   case 3: return index++, [3, 1]
    let input = r#"
function process_items(items) {
  return __awaiter(this, void 0, void 0, function () {
    var results, index, _a, _b;
    return __generator(this, function (_c) {
      switch (_c.label) {
        case 0:
          results = [], index = 0, _c.label = 1;
        case 1:
          return index < items.length ? (_b = (_a = results).push, [4, transform_item(items[index])]) : [3, 4];
        case 2:
          _b.apply(_a, [_c.sent()]), _c.label = 3;
        case 3:
          return index++, [3, 1];
        case 4:
          return [2, results];
      }
    });
  });
}
"#;
    let expected = r#"
async function process_items(items) {
  var results, index, _a, _b;
  results = [];
  index = 0;
  for (; index < items.length; index++) {
    results.push(await transform_item(items[index]));
  }
  return results;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn terser_compressed_generator_loop_full_pipeline() {
    // Full pipeline test: Terser-compressed __generator with inline helper body.
    // Earlier rules (SimplifySequence, UnConditionals) may modify the helper body
    // before UnAsyncAwait runs — this tests the realistic pipeline path.
    let input = r#"
var __generator=this&&this.__generator||function(thisArg,body){var _={label:0,sent:function(){if(1&t[0])throw t[1];return t[1]},trys:[],ops:[]},f,y,t,g=Object.create(("function"==typeof Iterator?Iterator:Object).prototype);return g.next=verb(0),g.throw=verb(1),g.return=verb(2),"function"==typeof Symbol&&(g[Symbol.iterator]=function(){return this}),g;function verb(n){return function(v){return step([n,v])}}function step(op){if(f)throw new TypeError("Generator is already executing.");for(;g&&(g=0,op[0]&&(_=0)),_;)try{if(f=1,y&&(t=2&op[0]?y.return:op[0]?y.throw||((t=y.return)&&t.call(y),0):y.next)&&!(t=t.call(y,op[1])).done)return t;switch(y=0,t&&(op=[2&op[0],t.value]),op[0]){case 0:case 1:t=op;break;case 4:return _.label++,{value:op[1],done:!1};case 5:_.label++,y=op[1],op=[0];continue;case 7:op=_.ops.pop(),_.trys.pop();continue;default:if(!(t=_.trys,(t=t.length>0&&t[t.length-1])||6!==op[0]&&2!==op[0])){_=0;continue}if(3===op[0]&&(!t||op[1]>t[0]&&op[1]<t[3])){_.label=op[1];break}if(6===op[0]&&_.label<t[1]){_.label=t[1],t=op;break}if(t&&_.label<t[2]){_.label=t[2],_.ops.push(op);break}t[2]&&_.ops.pop(),_.trys.pop();continue}op=body.call(thisArg,_)}catch(e){op=[6,e],y=0}finally{f=t=0}if(5&op[0])throw op[1];return{value:op[0]?op[1]:void 0,done:!0}}};function iter_items(items){var index;return __generator(this,function(_a){switch(_a.label){case 0:index=0,_a.label=1;case 1:return index<items.length?[4,items[index]]:[3,4];case 2:_a.sent(),_a.label=3;case 3:return index++,[3,1];case 4:return[2]}})}
"#;
    let output = render(input);
    assert!(
        output.contains("function*"),
        "should be a generator: {output}"
    );
    assert!(output.contains("yield"), "should contain yield: {output}");
    assert!(
        output.contains("for"),
        "should recover a for-loop: {output}"
    );
}

#[test]
fn awaiter_generator_recovers_nested_forward_branches() {
    // Preserve non-fallthrough forward gotos long enough for the shared
    // state-machine IR to recover nested branch joins.
    let input = r#"
function save(items) {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          if (!(items.length > 0)) return [3 /*break*/, 3];
          if (!useAsync) return [3 /*break*/, 2];
          return [4 /*yield*/, writeAsync(items)];
        case 1:
          _a.sent();
          return [3 /*break*/, 3];
        case 2:
          writeSync(items);
          _a.label = 3;
        case 3:
          return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function save(items) {
  if (items.length > 0) {
    if (useAsync) {
      await writeAsync(items);
    } else {
      writeSync(items);
    }
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
