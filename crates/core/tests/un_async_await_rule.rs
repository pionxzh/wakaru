mod common;

use common::{assert_eq_normalized, render, render_pipeline_between_with_facts, render_rule};
use wakaru_core::facts::{
    ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact, TypeScriptHelperKind,
};
use wakaru_core::rules::UnAsyncAwait;
use wakaru_core::validate_output_modules;

// ── __generator only ────────────────────────────────────────────────────────

fn apply(input: &str) -> String {
    let input = format!("{TS_HELPERS}\n{input}");
    render_rule(&input, UnAsyncAwait::new)
}

fn apply_without_helpers(input: &str) -> String {
    render_rule(input, UnAsyncAwait::new)
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
fn block_binding_sharing_the_state_name_keeps_its_own_sent_call() {
    // Unlike nested functions, blocks are traversed by the sent finder and
    // replacer. Their local `_a` must not consume the preceding yield.
    let input = r#"
function func() {
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        return [4 /*yield*/, load()];
      case 1:
        _a.sent();
        {
          let _a = item;
          use(_a.sent());
        }
        return [2 /*return*/];
    }
  });
}
"#;
    let expected = r#"
function* func() {
  yield load();
  {
    let _a = item;
    use(_a.sent());
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_preserves_callback_locals_declared_after_state_switch() {
    let input = r#"
const localValue = "module";
function loadValue() {
  return __generator(this, function (_state) {
    switch (_state.label) {
      case 0:
        localValue = createValue();
        return [2 /*return*/, localValue];
    }
    var localValue;
  });
}
"#;
    let expected = r#"
const localValue = "module";
function* loadValue() {
  var localValue;
  localValue = createValue();
  return localValue;
}
"#;
    assert_eq_normalized(&apply(input), expected);

    let pipeline_input = format!("{TS_HELPERS}\n{input}");
    let pipeline_output = render(&pipeline_input);
    assert!(
        pipeline_output.contains("let localValue;"),
        "the recovered callback local must remain a mutable function binding:\n{pipeline_output}"
    );
    let findings = validate_output_modules(&[("input.js".to_string(), pipeline_output.clone())]);
    assert!(
        findings.is_empty(),
        "the callback-local writes must not resolve to the module const:\n{pipeline_output}\n{findings:#?}"
    );
}

#[test]
fn generator_callback_local_colliding_with_destination_param_is_renamed() {
    // TypeScript `__generator` callback `var` shadows an outer parameter.
    // Flattening must rename the moved binding, not the parameter list.
    let input = r#"
function loadValue(localValue) {
  return __generator(this, function (_state) {
    switch (_state.label) {
      case 0:
        localValue = createValue();
        return [2 /*return*/, localValue];
    }
    var localValue;
  });
}
"#;
    let expected = r#"
function* loadValue(localValue) {
  var localValue1;
  localValue1 = createValue();
  return localValue1;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn awaiter_callback_local_colliding_with_destination_param_is_renamed() {
    let input = r#"
function loadValue(localValue) {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_state) {
      switch (_state.label) {
        case 0:
          localValue = createValue();
          return [2 /*return*/, localValue];
      }
      var localValue;
    });
  });
}
"#;
    let expected = r#"
async function loadValue(localValue) {
  var localValue1;
  localValue1 = createValue();
  return localValue1;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn awaiter_callback_var_colliding_with_unused_param_is_renamed() {
    // Reproduced with the repo's TypeScript 5 ES5 + Terser 5 mangle harness
    // from an async function with two parameters and two awaited locals.
    // Terser reuses an outer parameter name for a callback-local `var`; this
    // reduced form keeps the same cross-function binding collision.
    let input = r#"
function join(t, i, n, o, s, c) {
  return __awaiter(this, void 0, void 0, function () {
    var o, r;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [4 /*yield*/, send(c)];
        case 1:
          o = _a.sent();
          r = o;
          return [2 /*return*/, r];
      }
    });
  });
}
"#;
    let expected = r#"
async function join(t, i, n, o, s, c) {
  var o1, r;
  o1 = await send(c);
  r = o1;
  return r;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn awaiter_renames_only_moved_binding_when_outer_param_is_also_used() {
    let input = r#"
function join(o) {
  useOuter(o);
  return __awaiter(this, void 0, void 0, function () {
    var o;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          return [4 /*yield*/, send()];
        case 1:
          o = _a.sent();
          return [2 /*return*/, o];
      }
    });
  });
}
"#;
    let expected = r#"
async function join(o) {
  useOuter(o);
  var o1;
  o1 = await send();
  return o1;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn awaiter_callback_local_colliding_with_eval_fails_closed() {
    // Direct `eval` can observe the original callback-local name as a string.
    // Rename would rebind that string, so keep the function boundary.
    let input = r#"
function loadValue(localValue) {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_state) {
      switch (_state.label) {
        case 0:
          eval("localValue");
          localValue = createValue();
          return [2 /*return*/, localValue];
      }
      var localValue;
    });
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("function loadValue(localValue)")
            && output.contains("return async function()")
            && output.contains("var localValue;")
            && output.contains("eval(\"localValue\")"),
        "direct eval must keep the nested callback boundary and original local:\n{output}"
    );
    assert!(
        !output.contains("async function loadValue(localValue)"),
        "eval plus a colliding callback local must not flatten onto the parameter:\n{output}"
    );
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn awaiter_callback_local_collision_with_destination_eval_fails_closed() {
    // A rename to `localValue1` would introduce a hoisted binding that captures
    // the outer eval source. The eval is outside the callback being moved, so
    // both sides of the function-boundary move must participate in the guard.
    let input = r#"
function loadValue(localValue) {
  observe(eval("typeof localValue1"));
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_state) {
      switch (_state.label) {
        case 0:
          localValue = createValue();
          return [2 /*return*/, localValue];
      }
      var localValue;
    });
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("eval(\"typeof localValue1\")")
            && output.contains("return async function()")
            && output.contains("var localValue;"),
        "destination eval must preserve the nested callback boundary:\n{output}"
    );
    assert!(
        !output.contains("async function loadValue(localValue)"),
        "a renamed callback local must not capture a destination eval source:\n{output}"
    );
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn awaiter_callback_function_decl_name_collision_fails_closed() {
    let input = r#"
function loadValue(worker) {
  return __awaiter(this, void 0, void 0, function* () {
    function worker() {}
    observe(worker.name);
    yield ready();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return async function()")
            && output.contains("function worker()")
            && output.contains("observe(worker.name)"),
        "renaming a moved function declaration would change Function.name:\n{output}"
    );
    assert!(
        !output.contains("async function loadValue(worker)"),
        "an observable function declaration name must keep the callback boundary:\n{output}"
    );
}

#[test]
fn awaiter_callback_class_decl_name_collision_fails_closed() {
    let input = r#"
function loadValue(Worker) {
  return __awaiter(this, void 0, void 0, function* () {
    class Worker {}
    observe(Worker.name);
    yield ready();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return async function()")
            && output.contains("class Worker")
            && output.contains("observe(Worker.name)"),
        "renaming a moved class declaration would change its name:\n{output}"
    );
    assert!(
        !output.contains("async function loadValue(Worker)"),
        "an observable class declaration name must keep the callback boundary:\n{output}"
    );
}

#[test]
fn awaiter_callback_inferred_initializer_name_collision_fails_closed() {
    let input = r#"
function loadValue(task) {
  return __awaiter(this, void 0, void 0, function* () {
    var task = function () {};
    observe(task.name);
    yield ready();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return async function()")
            && output.contains("var task = function()")
            && output.contains("observe(task.name)"),
        "renaming a binding must not change an anonymous initializer's inferred name:\n{output}"
    );
    assert!(
        !output.contains("async function loadValue(task)"),
        "an inferred initializer name must keep the callback boundary:\n{output}"
    );
}

#[test]
fn awaiter_callback_named_initializer_collision_can_rename() {
    let input = r#"
function loadValue(task) {
  return __awaiter(this, void 0, void 0, function* () {
    var task = function namedTask() {};
    observe(task.name);
    yield ready();
  });
}
"#;
    let expected = r#"
async function loadValue(task) {
  var task1 = function namedTask() {};
  observe(task1.name);
  await ready();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn awaiter_callback_inferred_assignment_name_collision_fails_closed() {
    let input = r#"
function loadValue(task) {
  return __awaiter(this, void 0, void 0, function* () {
    var task;
    task ||= class {};
    observe(task.name);
    yield ready();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return async function()")
            && output.contains("task ||= class")
            && output.contains("observe(task.name)"),
        "renaming a logical-assignment target would change the class name:\n{output}"
    );
    assert!(
        !output.contains("async function loadValue(task)"),
        "an inferred assignment name must keep the callback boundary:\n{output}"
    );
}

#[test]
fn awaiter_callback_inferred_plain_assignment_name_collision_fails_closed() {
    let input = r#"
function loadValue(task) {
  return __awaiter(this, void 0, void 0, function* () {
    var task;
    task = function () {};
    observe(task.name);
    yield ready();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return async function()")
            && output.contains("task = function()")
            && output.contains("observe(task.name)"),
        "renaming a plain-assignment target would change the function name:\n{output}"
    );
    assert!(
        !output.contains("async function loadValue(task)"),
        "an inferred plain-assignment name must keep the callback boundary:\n{output}"
    );
}

#[test]
fn awaiter_callback_inferred_destructuring_default_name_collision_fails_closed() {
    let input = r#"
function loadValue(task) {
  return __awaiter(this, void 0, void 0, function* () {
    var { task = () => {} } = {};
    observe(task.name);
    yield ready();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return async function()")
            && output.contains("var { task =")
            && output.contains("observe(task.name)"),
        "renaming a destructuring binding would change its default arrow's name:\n{output}"
    );
    assert!(
        !output.contains("async function loadValue(task)"),
        "an inferred destructuring default name must keep the callback boundary:\n{output}"
    );
}

#[test]
fn generator_callback_with_observable_trailing_statement_fails_closed() {
    let input = r#"
function loadValue() {
  return __generator(this, function (_state) {
    switch (_state.label) {
      case 0:
        return [2 /*return*/, createValue()];
    }
    observeStateCallback();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("observeStateCallback();"),
        "a sibling statement must never be silently dropped:\n{output}"
    );
    assert!(
        output.contains("return __generator(this, function(_state)"),
        "an observable callback sibling must keep the state machine:\n{output}"
    );
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

#[test]
fn invalid_awaiter_extraction_preserves_surrounding_statements() {
    let input = r#"
function load() {
  const config = {
    nested: { values: [1, 2, 3] },
    read() { return this.nested.values; }
  };
  observe(config);
  return __awaiter(ctx.scope, void 0, void 0, function* () {
    yield work();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("const config = {"),
        "an invalid awaiter candidate must preserve preceding statements, got:\n{output}"
    );
    assert!(
        output.contains("observe(config)"),
        "an invalid awaiter candidate must preserve surrounding uses, got:\n{output}"
    );
    assert!(
        output.contains("return __awaiter(ctx.scope, void 0, void 0, function*()"),
        "an unsupported thisArg must preserve the awaiter wrapper, got:\n{output}"
    );
    assert!(
        !output.contains("async function load"),
        "an invalid awaiter candidate must not mark the function async, got:\n{output}"
    );
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
    // The nested wrapper passes `void 0`, not `this`: a body that read the
    // top-level `this` would preserve the outer wrapper for a different
    // reason (see the module-level this test below).
    let input = r#"
__awaiter(this, void 0, void 0, function* () {
  const nested = () => __generator(void 0, function (_a) {
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
        output.contains("=>__generator(void 0, function(_a)"),
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

// ── __awaiter canonical call frame ──────────────────────────────────────────

#[test]
fn awaiter_with_void_call_this_arg_is_preserved() {
    // `void probe()` is not tsc output; unwrapping would delete the
    // `probe()` evaluation.
    let input = r#"
function f() {
  return __awaiter(void probe(), void 0, void 0, function* () {
    yield work();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return __awaiter(void probe(), void 0, void 0, function*()"),
        "a side-effecting thisArg must preserve the awaiter wrapper, got:\n{output}"
    );
    assert!(
        !output.contains("async function f"),
        "a side-effecting thisArg must not be recovered as async, got:\n{output}"
    );
}

#[test]
fn awaiter_with_extra_arguments_is_preserved() {
    // tsc emits exactly four arguments; a fifth would be silently dropped.
    let input = r#"
function f() {
  return __awaiter(this, void 0, void 0, function* () {
    yield work();
  }, extra());
}
"#;
    let output = apply(input);
    assert!(
        output.contains("extra()"),
        "an extra argument must survive, got:\n{output}"
    );
    assert!(
        !output.contains("async function f"),
        "a five-argument call must not be recovered as async, got:\n{output}"
    );
}

#[test]
fn awaiter_with_custom_promise_constructor_is_preserved() {
    // A custom Promise slot means the returned promise's identity differs
    // from a native `async` function's; the wrapper must stay.
    let input = r#"
function f() {
  return __awaiter(this, void 0, CustomPromise, function* () {
    yield work();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("return __awaiter(this, void 0, CustomPromise, function*()"),
        "a custom Promise constructor must preserve the awaiter wrapper, got:\n{output}"
    );
    assert!(
        !output.contains("async function f"),
        "a custom Promise constructor must not be recovered as async, got:\n{output}"
    );
}

#[test]
fn awaiter_with_side_effecting_arguments_slot_is_preserved() {
    let input = r#"
function f() {
  return __awaiter(this, probe(), void 0, function* () {
    yield work();
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("probe()"),
        "a side-effecting arguments slot must survive, got:\n{output}"
    );
    assert!(
        !output.contains("async function f"),
        "a side-effecting arguments slot must not be recovered as async, got:\n{output}"
    );
}

#[test]
fn awaiter_with_canonical_void_frame_is_recovered() {
    let input = r#"
function f() {
  return __awaiter(this, void 0, void 0, function* () {
    yield work();
  });
}
"#;
    let expected = r#"
async function f() {
  await work();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn awaiter_with_global_promise_frame_is_recovered() {
    let input = r#"
function f() {
  return __awaiter(this, void 0, Promise, function* () {
    yield work();
  });
}
"#;
    let expected = r#"
async function f() {
  await work();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_awaiter_with_arbitrary_arguments_identifier() {
    // A non-`arguments` identifier in the arguments slot applies real values
    // to the generator; unwrapping would discard them (and an undeclared name
    // would even lose its ReferenceError).
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f() {
    return __awaiter(this, supplied, void 0, function* (x) {
        yield x;
    });
}
"#;
    let output = render(input);
    assert!(
        output.contains("__awaiter") && output.contains("supplied"),
        "non-canonical arguments slot must preserve the wrapper:\n{output}"
    );
}

#[test]
fn preserves_awaiter_with_parameterized_generator() {
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f() {
    return __awaiter(this, arguments, void 0, function* (x) {
        yield x;
    });
}
"#;
    let output = render(input);
    assert!(
        output.contains("__awaiter"),
        "parameterized generator must preserve the wrapper:\n{output}"
    );
}

#[test]
fn shadowed_undefined_this_arg_is_treated_as_an_alias() {
    // A local binding named `undefined` carries a value; splicing the body
    // must not silently rebind `this` to the enclosing one.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f(undefined) {
    return __awaiter(undefined, void 0, void 0, function* () {
        yield this.x;
    });
}
"#;
    let output = render(input);
    assert!(
        !output.contains("await this.x"),
        "shadowed undefined must not become the enclosing this:\n{output}"
    );
}

#[test]
fn preserves_awaiter_with_unresolved_this_alias() {
    // An undeclared thisArg identifier throws ReferenceError when evaluated;
    // if the generator body never reads `this`, unwrapping would delete that
    // throw entirely. Only resolver-proven local aliases take the alias path.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f() {
    return __awaiter(missing, void 0, void 0, function* () {
        yield work();
    });
}
"#;
    let output = render(input);
    assert!(
        output.contains("__awaiter") && output.contains("missing"),
        "unresolved thisArg must preserve the wrapper:\n{output}"
    );
}

#[test]
fn isolated_rule_preserves_unresolved_this_alias() {
    // The standalone rule now carries the resolver mark, so the isolated
    // path applies the same fail-closed alias proof as the pipeline.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f() {
    return __awaiter(missing, void 0, void 0, function* () {
        yield work();
    });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("__awaiter") && output.contains("missing"),
        "unresolved thisArg must preserve the wrapper in the isolated rule:\n{output}"
    );
}

#[test]
fn isolated_rule_treats_shadowed_undefined_as_an_alias() {
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f(undefined) {
    return __awaiter(undefined, void 0, void 0, function* () {
        yield this.x;
    });
}
"#;
    let output = apply(input);
    assert!(
        !output.contains("await this.x"),
        "shadowed undefined must not become the enclosing this in the isolated rule:\n{output}"
    );
}

// ── thisArg / arguments slots must survive the splice destination ──────────

#[test]
fn recovers_arguments_slot_when_the_spliced_body_reads_arguments() {
    // tsc's canonical pair: the `arguments` slot with a body that reads
    // `arguments`. Splicing into the enclosing function keeps the same
    // arguments object.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function h() {
    return __awaiter(this, arguments, void 0, function* () {
        yield arguments[0];
    });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("async function h()") && output.contains("await arguments[0]"),
        "the canonical arguments pair must recover:\n{output}"
    );
}

#[test]
fn return_path_preserves_wrapper_when_empty_arguments_body_reads_arguments() {
    // `void 0` applies an empty arguments list, but the spliced body would
    // read the enclosing function's real arguments.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function h() {
    return __awaiter(this, void 0, void 0, function* () {
        yield arguments[0];
    });
}
"#;
    let output = apply(input);
    assert!(
        !output.contains("async function h()"),
        "an empty arguments slot must not splice a body that reads arguments:\n{output}"
    );
}

#[test]
fn return_path_preserves_wrapper_when_undefined_this_arg_body_reads_this() {
    // `void 0` binds `this` to undefined inside the generator; splicing into
    // `g` would rebind it to g's receiver.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function g() {
    return __awaiter(void 0, void 0, void 0, function* () {
        yield this.x;
    });
}
"#;
    let output = apply(input);
    assert!(
        !output.contains("async function g()"),
        "a void thisArg must not splice a body that reads this into the enclosing function:\n{output}"
    );
    // The expression-position fallback is exact here: a receiver-less IIFE
    // also sees `this === undefined`.
    assert!(
        output.contains("async function()") && output.contains("await this.x"),
        "the IIFE form reproduces the undefined receiver:\n{output}"
    );
}

#[test]
fn iife_path_preserves_wrapper_when_this_arg_body_reads_this_inside_a_function() {
    // Inside `f`, `this` is f's receiver; a fresh IIFE would see undefined.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f() {
    return [1].map(() => __awaiter(this, void 0, void 0, function* () {
        yield this.x;
    }));
}
"#;
    let output = apply(input);
    assert!(
        output.contains("__awaiter(this") && output.contains("yield this.x"),
        "an IIFE would rebind this; the wrapper must stay:\n{output}"
    );
}

#[test]
fn iife_path_preserves_wrapper_when_top_level_this_arg_body_reads_this() {
    // A module's top-level `this` is undefined like the IIFE's, but wakaru
    // preserves the script goal and a strict script's top-level `this` is the
    // global object — the receiver-less IIFE would change it. No depth-zero
    // exception: fail closed.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
__awaiter(this, void 0, void 0, function* () {
    yield this.x;
});
"#;
    let output = apply(input);
    assert!(
        output.contains("__awaiter(this") && output.contains("yield this.x"),
        "top-level this must not be rebound by an IIFE:\n{output}"
    );
}

#[test]
fn iife_path_preserves_wrapper_when_arguments_slot_body_reads_arguments() {
    // The `arguments` slot forwards f's arguments; a fresh IIFE receives none.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f() {
    run(__awaiter(this, arguments, void 0, function* () {
        yield arguments[0];
    }));
}
"#;
    let output = apply(input);
    assert!(
        output.contains("__awaiter(this, arguments"),
        "an IIFE would empty arguments; the wrapper must stay:\n{output}"
    );
}

#[test]
fn preserves_wrapper_when_this_alias_is_written() {
    // The helper reads `_this` once at call time; the spliced body would read
    // it live, after the reassignment.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function k() {
    var _this = this;
    _this = other;
    return __awaiter(_this, void 0, void 0, function* () {
        yield this.x;
    });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("__awaiter(_this") && !output.contains("async function k()"),
        "a written alias must preserve the wrapper:\n{output}"
    );
}

#[test]
fn this_alias_rewrites_lexical_this_in_class_extends_and_computed_keys() {
    // `extends` expressions and computed keys evaluate in the enclosing scope
    // and must follow the alias; method bodies keep their own `this`.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
function f(ctx) {
    return __awaiter(ctx, void 0, void 0, function* () {
        const C = class extends this.Base {
            [this.k]() { return this; }
        };
        yield C;
    });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("extends ctx.Base") && output.contains("[ctx.k]"),
        "lexical this inside the class must follow the alias:\n{output}"
    );
    assert!(
        output.contains("return this;"),
        "the method body keeps its own this:\n{output}"
    );
}

// ── with statements make identifier-shaped frame slots untrustworthy ────────

#[test]
fn with_statement_preserves_wrapper_with_identifier_this_arg() {
    // Inside `with (box)`, `undefined` may resolve to `box.undefined` at
    // runtime and carry a real receiver; the resolver cannot see that. The
    // module-wide check refuses every identifier-shaped slot.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
var box = { undefined: ctx };
with (box) {
    __awaiter(undefined, void 0, void 0, function* () {
        yield this.x;
    });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("__awaiter(undefined") && !output.contains("async function"),
        "an identifier thisArg under a with statement must keep the wrapper:\n{output}"
    );
}

#[test]
fn with_statement_still_recovers_literal_frame_slots() {
    // `this` and `void <literal>` are not name lookups, so a `with` elsewhere
    // in the module does not affect them.
    let input = r#"
var __awaiter = require("tslib").__awaiter;
with (box) { use(value); }
function f() {
    return __awaiter(this, void 0, void 0, function* () {
        yield work();
    });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("async function f()") && output.contains("await work()"),
        "literal-shaped slots stay canonical under a with statement:\n{output}"
    );
}

#[test]
fn with_statement_rejects_identifier_arguments_and_promise_slots() {
    let input = r#"
var __awaiter = require("tslib").__awaiter;
with (box) { use(value); }
function g() {
    return __awaiter(this, arguments, Promise, function* () {
        yield work();
    });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("__awaiter(this, arguments, Promise") && !output.contains("async function"),
        "identifier arguments/Promise slots under a with statement must keep the wrapper:\n{output}"
    );
}

// TypeScript 5.9.3 --importHelpers emits these call frames (ES2015/ES5).
fn tslib_async_body(awaiter: &str, generator: Option<&str>) -> String {
    let body = generator.map_or_else(
        || "function* () { var result = yield value; return result + 1; }".to_string(),
        |generator| {
            format!(
                r#"function () {{
            var result;
            return {generator}(this, function (state) {{
                switch (state.label) {{
                    case 0: return [4, value];
                    case 1: result = state.sent(); return [2, result + 1];
                }}
            }});
        }}"#
            )
        },
    );
    format!("function load(value) {{ return {awaiter}(this, void 0, void 0, {body}); }}")
}

#[test]
fn tslib_namespace_async_helpers_restore_both_targets() {
    for declaration in [
        "var runtime = require(\"tslib\");",
        "import * as runtime from \"tslib\";",
        "import runtime from \"tslib\";",
        "import * as runtime from \"tslib/tslib.es6.js\";",
    ] {
        for generator in [None, Some("runtime.__generator")] {
            let input = format!(
                "{declaration}\n{}",
                tslib_async_body("runtime.__awaiter", generator)
            );
            let statements = if generator.is_some() {
                "var result; result = await value; return result + 1;"
            } else {
                "var result = await value; return result + 1;"
            };
            let expected = format!("{declaration}\nasync function load(value) {{ {statements} }}");
            assert_eq_normalized(&apply_without_helpers(&input), &expected);
        }
    }
}

#[test]
fn tslib_direct_require_member_async_helpers_restore_both_targets() {
    for generator in [None, Some("require(\"tslib\").__generator")] {
        let input = tslib_async_body("require(\"tslib\").__awaiter", generator);
        let statements = if generator.is_some() {
            "var result; result = await value; return result + 1;"
        } else {
            "var result = await value; return result + 1;"
        };
        assert_eq_normalized(
            &apply_without_helpers(&input),
            &format!("async function load(value) {{ {statements} }}"),
        );
    }
}

#[test]
fn tslib_mixed_alias_and_namespace_return_the_awaited_value() {
    for (alias, member, awaiter, generator) in [
        ("runAsync", "__awaiter", "runAsync", "runtime.__generator"),
        (
            "runGenerator",
            "__generator",
            "runtime.__awaiter",
            "runGenerator",
        ),
    ] {
        let declaration =
            format!("var runtime = require(\"tslib\"); var {alias} = runtime.{member};");
        let input = format!(
            "{declaration}\n{}",
            tslib_async_body(awaiter, Some(generator))
        );
        // In particular, an async function returning runtime.__generator is
        // NOT a successful recovery: its resolved value would be an iterator.
        let expected = format!(
            r#"{declaration}
            async function load(value) {{
                var result;
                result = await value;
                return result + 1;
            }}"#
        );
        assert_eq_normalized(&apply_without_helpers(&input), &expected);
        let output = render(&input);
        assert!(output.contains("async function load(value)"), "{output}");
        assert!(output.contains("await value"), "{output}");
        assert!(
            !output.contains("__generator("),
            "must return the value, not an iterator: {output}"
        );
    }
}

#[test]
fn tslib_namespace_members_require_matching_binding_and_source() {
    for declaration in [
        "import * as runtime from \"./other.js\";",
        "var runtime = require(\"other\");",
        "var runtime = customRuntime;",
    ] {
        let input = format!(
            "{declaration}\n{}",
            tslib_async_body("runtime.__awaiter", Some("runtime.__generator"))
        );
        assert_eq_normalized(&apply_without_helpers(&input), &input);
    }
    let shadowed = format!(
        "import * as runtime from \"tslib\"; function wrapper(runtime) {{ {} return load; }}",
        tslib_async_body("runtime.__awaiter", Some("runtime.__generator"))
    );
    assert_eq_normalized(&apply_without_helpers(&shadowed), &shadowed);
}

#[test]
fn tslib_async_members_do_not_trust_shadowed_require() {
    for body in [
        format!(
            "var runtime = require(\"tslib\"); {}",
            tslib_async_body("runtime.__awaiter", Some("runtime.__generator"))
        ),
        tslib_async_body(
            "require(\"tslib\").__awaiter",
            Some("require(\"tslib\").__generator"),
        ),
    ] {
        let input = format!("function require(name) {{ return customRuntime; }} {body}");
        assert_eq_normalized(&apply_without_helpers(&input), &input);
        assert!(!render(&input).contains("async function load"));
    }
}

#[test]
fn tslib_namespace_async_rollback_keeps_unsupported_generator() {
    let input = r#"
        import * as runtime from "tslib";
        function load(value) {
            return runtime.__awaiter(this, void 0, void 0, function () {
                return runtime.__generator(this, function (state) {
                    switch (state.label) {
                        case 0: return [3, 99];
                        case 1: return [2, value];
                    }
                });
            });
        }
    "#;
    assert_eq_normalized(&apply_without_helpers(input), input);
    assert!(!render(input).contains("async function load"));
}

#[test]
fn tslib_namespace_awaiter_keeps_noncanonical_frame() {
    let input = format!(
        "import * as runtime from \"tslib\"; {}",
        tslib_async_body("runtime.__awaiter", None)
    )
    .replace("this, void 0, void 0", "this, void 0, CustomPromise");
    assert_eq_normalized(&apply_without_helpers(&input), &input);
}

#[test]
fn tslib_namespace_awaiter_restores_expression_position() {
    let input = r#"
        import * as runtime from "tslib";
        consume(runtime.__awaiter(void 0, void 0, void 0, function* () {
            return yield ready();
        }));
    "#;
    let expected = r#"
        import * as runtime from "tslib";
        consume(async function () { return await ready(); }());
    "#;
    assert_eq_normalized(&apply_without_helpers(input), expected);
}

#[test]
fn tslib_namespace_members_preserve_with_lookup() {
    for callee in ["runtime.__awaiter", "require(\"tslib\").__awaiter"] {
        let input = format!(
            r#"
            var runtime = require("tslib");
            with (scope) {{
                consume({callee}(void 0, void 0, void 0, function* () {{
                    return yield ready();
                }}));
            }}
        "#
        );
        assert_eq_normalized(&apply_without_helpers(&input), &input);
    }
}

#[test]
fn delegated_values_restore_across_tslib_delivery_forms() {
    for (prefix, values) in [
        ("import * as ts from 'tslib';", "ts.__values"),
        ("var ts = require('tslib');", "ts.__values"),
        ("import { __values as v } from 'tslib';", "v"),
        ("", "require('tslib').__values"),
    ] {
        let input = format!(
            "{prefix} function read(items) {{ return __generator(this, function(state) {{ return [5, {values}(items)]; }}); }}"
        );
        let output = apply(&input);
        assert!(output.contains("yield* items"), "{output}");
    }
}

#[test]
fn delegated_values_preserve_unknown_calls_and_argument_effects() {
    for (params, value) in [
        ("items, __values", "__values(items)"),
        ("items, require", "require('tslib').__values(items)"),
        ("items, ts", "ts.__values(items)"),
        ("items", "ts.__values(items, effect())"),
        ("items", "ts.__values(...items)"),
    ] {
        let input = format!(
            "import * as ts from 'tslib'; function read({params}) {{ return __generator(this, function(state) {{ return [5, {value}]; }}); }}"
        );
        let output = apply(&input);
        assert!(!output.contains("yield* items;"), "{output}");
        assert!(
            output.contains(".__values(") || output.contains("yield* __values("),
            "{output}"
        );
    }
}

#[test]
fn delegated_values_preserve_reassigned_helpers() {
    for (prefix, value) in [
        ("var v = require('tslib').__values; v = custom;", "v"),
        ("var ts = require('tslib'); ts = custom;", "ts.__values"),
    ] {
        let input = format!(
            "{prefix} function read(items) {{ return __generator(this, function(state) {{ return [5, {value}(items)]; }}); }}"
        );
        assert!(!apply(&input).contains("yield* items;"));
    }
}

#[test]
fn delegated_values_use_cross_module_namespace_facts() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            ts_helper_exports: vec![
                TypeScriptHelperExportFact {
                    exported: "g".into(),
                    local: Some("g".into()),
                    kind: TypeScriptHelperKind::Generator,
                },
                TypeScriptHelperExportFact {
                    exported: "v".into(),
                    local: Some("v".into()),
                    kind: TypeScriptHelperKind::Values,
                },
            ],
            ..Default::default()
        },
    );
    let input = r#"
import * as h from "./helpers.js";
function read(items) {
    return h.g(this, function(state) { return [5, h.v(items)]; });
}
"#;
    assert!(apply_cross_module_facts(input, &facts).contains("yield* items"));
}

#[test]
fn catch_binding_avoids_names_the_machine_already_spells() {
    // The catch body reads an outer `error`, so the synthesized catch parameter
    // must not take that spelling. The lowered alias `error_1` is folded into
    // the binding, so its spelling is free to reuse.
    let input = r#"
function fetch_items(source, error) {
  var error_1;
  return __generator(this, function (_a) {
    switch (_a.label) {
      case 0:
        _a.trys.push([0, 2, , 3]);
        return [4 /*yield*/, start_fetch(source)];
      case 1:
        _a.sent();
        return [3 /*break*/, 3];
      case 2:
        error_1 = _a.sent();
        handle(error_1, error);
        return [3 /*break*/, 3];
      case 3:
        return [2 /*return*/];
    }
  });
}
"#;
    let expected = r#"
function* fetch_items(source, error) {
  var error_1;
  try {
    yield start_fetch(source);
  } catch (error_1) {
    handle(error_1, error);
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ── try regions entered through a conditional jump ──────────────────────────

#[test]
fn guarded_try_catch_stays_inside_its_branch() {
    // TypeScript ES5 output for `if (loader.lazy) { try { await ... } catch
    // (error) { ... } } else { ... }`. The try region starts at label 1, which
    // is reached only by falling through the guard in label 0. Folding the
    // branches must keep the try/catch inside the guarded branch; dropping it
    // runs the catch body unconditionally after the await.
    let input = r#"
function load_resource(loader, path, options) {
  return __awaiter(this, void 0, void 0, function () {
    var error_1;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          if (!loader.lazy) return [3 /*break*/, 5];
          _a.label = 1;
        case 1:
          _a.trys.push([1, 3, , 4]);
          return [4 /*yield*/, loader.load(path, options)];
        case 2:
          _a.sent();
          return [3 /*break*/, 4];
        case 3:
          error_1 = _a.sent();
          report_error(error_1);
          return [3 /*break*/, 4];
        case 4: return [3 /*break*/, 6];
        case 5:
          loader.load(path, options).catch(report_error);
          _a.label = 6;
        case 6: return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function load_resource(loader, path, options) {
  var error_1;
  if (loader.lazy) {
    try {
      await loader.load(path, options);
    } catch (error) {
      report_error(error);
    }
  } else {
    loader.load(path, options).catch(report_error);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn guarded_try_catch_in_terser_conditional_return_stays_inside_its_branch() {
    // Terser folds the guard and the else branch into one conditional return:
    // `return e.type ? [3, 1] : (else_work, [3, 4])`. The try region at labels
    // 1-3 is the taken branch of that conditional.
    let input = r#"
function load(e, t, n) {
  return __awaiter(this, void 0, void 0, function () {
    var i;
    return __generator(this, function (s) {
      switch (s.label) {
        case 0: return e.type ? [3, 1] : (e.load(t, n).catch(handle), [3, 4]);
        case 1: s.trys.push([1, 3, , 4]); return [4, e.load(t, n)];
        case 2: s.sent(); return [3, 4];
        case 3: i = s.sent(); handle(i); return [3, 4];
        case 4: return [2];
      }
    });
  });
}
"#;
    let expected = r#"
async function load(e, t, n) {
  var i;
  if (!e.type) {
    e.load(t, n).catch(handle);
  } else {
    try {
      await e.load(t, n);
    } catch (error) {
      handle(error);
    }
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn guarded_try_catch_without_else_is_recovered() {
    // Same region, but the guard jumps straight to the end of the machine.
    let input = r#"
function load_resource(loader, path, options) {
  return __awaiter(this, void 0, void 0, function () {
    var error_1;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0:
          if (!loader.lazy) return [3 /*break*/, 4];
          _a.label = 1;
        case 1:
          _a.trys.push([1, 3, , 4]);
          return [4 /*yield*/, loader.load(path, options)];
        case 2:
          _a.sent();
          return [3 /*break*/, 4];
        case 3:
          error_1 = _a.sent();
          report_error(error_1);
          return [3 /*break*/, 4];
        case 4: return [2 /*return*/];
      }
    });
  });
}
"#;
    let expected = r#"
async function load_resource(loader, path, options) {
  var error_1;
  if (loader.lazy) {
    try {
      await loader.load(path, options);
    } catch (error) {
      report_error(error);
    }
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    let findings = validate_output_modules(&[("input.js".to_string(), output)]);
    assert!(findings.is_empty(), "{findings:#?}");
}

// ── Inline `__values` detection must not swallow user functions ─────────────

#[test]
fn one_param_function_with_a_nested_iterable_helper_is_not_a_values_helper() {
    // A single-parameter user function whose body contains an inlined Babel
    // iterable helper (`Symbol.iterator`) and a `TypeError` throw shares the
    // `__values` signals only inside nested functions. It is not a helper and
    // must survive even when nothing references it: dead input code is kept.
    let input = r#"
var C = function(r) {
  var t = r.reason.stack;
  if (t) {
    var o = function(r) {
      var n = r == null ? null : typeof Symbol !== "undefined" && r[Symbol.iterator] || r["@@iterator"];
      if (n != null) return n.call(r);
    }(t.match(E)) || function() {
      throw new TypeError("Invalid attempt to destructure non-iterable instance.");
    }();
    report(o);
  }
};
"#;
    assert_eq_normalized(&apply_without_helpers(input), input);
}

#[test]
fn function_referenced_only_inside_another_misclassified_function_survives() {
    // Bench shape: `C` and `R` both carry the loose `__values` signals through
    // nested inlined helpers. `C` is only referenced inside `R`'s initializer;
    // `R` stays because the module calls it, so `C` must stay as well.
    let input = r#"
var C = function(r) {
  var t = r.reason.stack;
  if (t) {
    var o = function(r) {
      var n = r == null ? null : typeof Symbol !== "undefined" && r[Symbol.iterator] || r["@@iterator"];
      if (n != null) return n.call(r);
    }(t.match(E)) || function() {
      throw new TypeError("Invalid attempt to destructure non-iterable instance.");
    }();
    report(o);
  }
};
var R = (r) => {
  var items = function(r) {
    if (typeof Symbol !== "undefined" && r[Symbol.iterator] != null) return Array.from(r);
  }(r) || function() {
    throw new TypeError("Invalid attempt to spread non-iterable instance.");
  }();
  window.addEventListener("unhandledrejection", C);
  return items;
};
R([]);
"#;
    assert_eq_normalized(&apply_without_helpers(input), input);
}

// ── for await through the __generator machine ───────────────────────────────

#[test]
fn ts_es5_for_await_recovers_through_the_state_machine() {
    // TypeScript ES5 output for `for await (const item of stream) { await
    // handle_item(item); }`: the loop is a back-edge inside a try region of the
    // `__generator` machine, and its protocol is folded by UnForOf afterwards.
    let input = r#"
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __generator = (this && this.__generator) || function (thisArg, body) {
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g = Object.create((typeof Iterator === "function" ? Iterator : Object).prototype);
    return g.next = verb(0), g["throw"] = verb(1), g["return"] = verb(2), typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
    function verb(n) { return function (v) { return step([n, v]); }; }
    function step(op) {
        if (f) throw new TypeError("Generator is already executing.");
        while (g && (g = 0, op[0] && (_ = 0)), _) try {
            if (f = 1, y && (t = op[0] & 2 ? y["return"] : op[0] ? y["throw"] || ((t = y["return"]) && t.call(y), 0) : y.next) && !(t = t.call(y, op[1])).done) return t;
            if (y = 0, t) op = [op[0] & 2, t.value];
            switch (op[0]) {
                case 0: case 1: t = op; break;
                case 4: _.label++; return { value: op[1], done: false };
                case 5: _.label++; y = op[1]; op = [0]; continue;
                case 7: op = _.ops.pop(); _.trys.pop(); continue;
                default:
                    if (!(t = _.trys, t = t.length > 0 && t[t.length - 1]) && (op[0] === 6 || op[0] === 2)) { _ = 0; continue; }
                    if (op[0] === 3 && (!t || (op[1] > t[0] && op[1] < t[3]))) { _.label = op[1]; break; }
                    if (op[0] === 6 && _.label < t[1]) { _.label = t[1]; t = op; break; }
                    if (t && _.label < t[2]) { _.label = t[2]; _.ops.push(op); break; }
                    if (t[2]) _.ops.pop();
                    _.trys.pop(); continue;
            }
            op = body.call(thisArg, _);
        } catch (e) { op = [6, e]; y = 0; } finally { f = t = 0; }
        if (op[0] & 5) throw op[1]; return { value: op[0] ? op[1] : void 0, done: true };
    }
};
var __asyncValues = (this && this.__asyncValues) || function (o) {
    if (!Symbol.asyncIterator) throw new TypeError("Symbol.asyncIterator is not defined.");
    var m = o[Symbol.asyncIterator], i;
    return m ? m.call(o) : (o = typeof __values === "function" ? __values(o) : o[Symbol.iterator](), i = {}, verb("next"), verb("throw"), verb("return"), i[Symbol.asyncIterator] = function () { return this; }, i);
    function verb(n) { i[n] = o[n] && function (v) { return new Promise(function (resolve, reject) { v = o[n](v), settle(resolve, reject, v.done, v.value); }); }; }
    function settle(resolve, reject, d, v) { Promise.resolve(v).then(function(v) { resolve({ value: v, done: d }); }, reject); }
};
function consume_stream(stream) {
    return __awaiter(this, void 0, void 0, function () {
        var item, e_1_1;
        var _a, stream_1, stream_1_1;
        var _b, e_1, _c, _d;
        return __generator(this, function (_e) {
            switch (_e.label) {
                case 0:
                    _e.trys.push([0, 6, 7, 12]);
                    _a = true, stream_1 = __asyncValues(stream);
                    _e.label = 1;
                case 1: return [4 /*yield*/, stream_1.next()];
                case 2:
                    if (!(stream_1_1 = _e.sent(), _b = stream_1_1.done, !_b)) return [3 /*break*/, 5];
                    _d = stream_1_1.value;
                    _a = false;
                    item = _d;
                    return [4 /*yield*/, handle_item(item)];
                case 3:
                    _e.sent();
                    _e.label = 4;
                case 4:
                    _a = true;
                    return [3 /*break*/, 1];
                case 5: return [3 /*break*/, 12];
                case 6:
                    e_1_1 = _e.sent();
                    e_1 = { error: e_1_1 };
                    return [3 /*break*/, 12];
                case 7:
                    _e.trys.push([7, , 10, 11]);
                    if (!(!_a && !_b && (_c = stream_1.return))) return [3 /*break*/, 9];
                    return [4 /*yield*/, _c.call(stream_1)];
                case 8:
                    _e.sent();
                    _e.label = 9;
                case 9: return [3 /*break*/, 11];
                case 10:
                    if (e_1) throw e_1.error;
                    return [7 /*endfinally*/];
                case 11: return [7 /*endfinally*/];
                case 12: return [2 /*return*/];
            }
        });
    });
}
"#;
    let expected = r#"
async function consume_stream(stream) {
  for await (const item of stream) {
    await handle_item(item);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn compressed_ts_for_await_folds_the_head_statement_form() {
    // Terser splits the `sent()` consumer from the guard, so the decoded loop
    // head is a statement of its own (`l = await i.next()`) at the back-edge
    // target. It must run every iteration, and the protocol still folds into
    // `for await` from that head-statement form.
    let input = r#"
var e=this&&this.__awaiter||function(e,t,n,r){function o(e){return e instanceof n?e:new n(function(t){t(e)})}return new(n||(n=Promise))(function(n,a){function u(e){try{s(r.next(e))}catch(e){a(e)}}function c(e){try{s(r.throw(e))}catch(e){a(e)}}function s(e){e.done?n(e.value):o(e.value).then(u,c)}s((r=r.apply(e,t||[])).next())})},t=this&&this.__generator||function(e,t){var n={label:0,sent:function(){if(1&a[0])throw a[1];return a[1]},trys:[],ops:[]},r,o,a,u=Object.create(("function"==typeof Iterator?Iterator:Object).prototype);return u.next=c(0),u.throw=c(1),u.return=c(2),"function"==typeof Symbol&&(u[Symbol.iterator]=function(){return this}),u;function c(e){return function(t){return s([e,t])}}function s(c){if(r)throw new TypeError("Generator is already executing.");for(;u&&(u=0,c[0]&&(n=0)),n;)try{if(r=1,o&&(a=2&c[0]?o.return:c[0]?o.throw||((a=o.return)&&a.call(o),0):o.next)&&!(a=a.call(o,c[1])).done)return a;switch(o=0,a&&(c=[2&c[0],a.value]),c[0]){case 0:case 1:a=c;break;case 4:return n.label++,{value:c[1],done:!1};case 5:n.label++,o=c[1],c=[0];continue;case 7:c=n.ops.pop(),n.trys.pop();continue;default:if(!(a=n.trys,(a=a.length>0&&a[a.length-1])||6!==c[0]&&2!==c[0])){n=0;continue}if(3===c[0]&&(!a||c[1]>a[0]&&c[1]<a[3])){n.label=c[1];break}if(6===c[0]&&n.label<a[1]){n.label=a[1],a=c;break}if(a&&n.label<a[2]){n.label=a[2],n.ops.push(c);break}a[2]&&n.ops.pop(),n.trys.pop();continue}c=t.call(e,n)}catch(e){c=[6,e],o=0}finally{r=a=0}if(5&c[0])throw c[1];return{value:c[0]?c[1]:void 0,done:!0}}},n=this&&this.__asyncValues||function(e){if(!Symbol.asyncIterator)throw new TypeError("Symbol.asyncIterator is not defined.");var t=e[Symbol.asyncIterator],n;return t?t.call(e):(e="function"==typeof __values?__values(e):e[Symbol.iterator](),n={},r("next"),r("throw"),r("return"),n[Symbol.asyncIterator]=function(){return this},n);function r(t){n[t]=e[t]&&function(n){return new Promise(function(r,a){o(r,a,(n=e[t](n)).done,n.value)})}}function o(e,t,n,r){Promise.resolve(r).then(function(t){e({value:t,done:n})},t)}};function r(r){return e(this,void 0,void 0,function(){var e,o,a,u,c,s,i,l,f,h,y,p;return t(this,function(t){switch(t.label){case 0:e=[],t.label=1;case 1:t.trys.push([1,,15,17]),t.label=2;case 2:t.trys.push([2,8,9,14]),s=!0,i=n(r),t.label=3;case 3:return[4,i.next()];case 4:return l=t.sent(),(f=l.done)?[3,7]:(p=l.value,s=!1,(o=p).done?[3,7]:(u=(a=e).push,[4,normalize_item(o)]));case 5:u.apply(a,[t.sent()]),t.label=6;case 6:return s=!0,[3,3];case 7:return[3,14];case 8:return c=t.sent(),h={error:c},[3,14];case 9:return t.trys.push([9,,12,13]),s||f||!(y=i.return)?[3,11]:[4,y.call(i)];case 10:t.sent(),t.label=11;case 11:return[3,13];case 12:if(h)throw h.error;return[7];case 13:return[7];case 14:return[3,17];case 15:return[4,close_stream(r)];case 16:return t.sent(),[7];case 17:return[2,e]}})})}
"#;
    let expected = r#"
async function r(r) {
  const e = [];
  try {
    for await (const o of r) {
      if (o.done) break;
      e.push(await normalize_item(o));
    }
  } finally {
    await close_stream(r);
  }
  return e;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

// ── early return inside a loop body ─────────────────────────────────────────

#[test]
fn ts_es5_infinite_loop_with_early_return_keeps_the_loop() {
    // `for (;;) { const r = await it.next(); if (await check(r)) return r; }`:
    // the `return` is a value-return opcode nested in the branch, and the
    // back-edge targets the machine entry (label 0). Both used to defeat the
    // decode; dropping the back-edge alone would run the body once.
    let input = r#"
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __generator = (this && this.__generator) || function (thisArg, body) {
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g = Object.create((typeof Iterator === "function" ? Iterator : Object).prototype);
    return g.next = verb(0), g["throw"] = verb(1), g["return"] = verb(2), typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
    function verb(n) { return function (v) { return step([n, v]); }; }
    function step(op) {
        if (f) throw new TypeError("Generator is already executing.");
        while (g && (g = 0, op[0] && (_ = 0)), _) try {
            if (f = 1, y && (t = op[0] & 2 ? y["return"] : op[0] ? y["throw"] || ((t = y["return"]) && t.call(y), 0) : y.next) && !(t = t.call(y, op[1])).done) return t;
            if (y = 0, t) op = [op[0] & 2, t.value];
            switch (op[0]) {
                case 0: case 1: t = op; break;
                case 4: _.label++; return { value: op[1], done: false };
                case 5: _.label++; y = op[1]; op = [0]; continue;
                case 7: op = _.ops.pop(); _.trys.pop(); continue;
                default:
                    if (!(t = _.trys, t = t.length > 0 && t[t.length - 1]) && (op[0] === 6 || op[0] === 2)) { _ = 0; continue; }
                    if (op[0] === 3 && (!t || (op[1] > t[0] && op[1] < t[3]))) { _.label = op[1]; break; }
                    if (op[0] === 6 && _.label < t[1]) { _.label = t[1]; t = op; break; }
                    if (t && _.label < t[2]) { _.label = t[2]; _.ops.push(op); break; }
                    if (t[2]) _.ops.pop();
                    _.trys.pop(); continue;
            }
            op = body.call(thisArg, _);
        } catch (e) { op = [6, e]; y = 0; } finally { f = t = 0; }
        if (op[0] & 5) throw op[1]; return { value: op[0] ? op[1] : void 0, done: true };
    }
};
function f(it, check) {
    return __awaiter(this, void 0, void 0, function () {
        var r;
        return __generator(this, function (_a) {
            switch (_a.label) {
                case 0: return [4 /*yield*/, it.next()];
                case 1:
                    r = _a.sent();
                    return [4 /*yield*/, check(r)];
                case 2:
                    if (_a.sent())
                        return [2 /*return*/, r];
                    _a.label = 3;
                case 3: return [3 /*break*/, 0];
                case 4: return [2 /*return*/];
            }
        });
    });
}
"#;
    let expected = r#"
async function f(it, check) {
  let r;
  for (;;) {
    r = await it.next();
    if (await check(r)) {
      return r;
    }
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn ts_es5_infinite_loop_with_early_return_inside_try_finally() {
    let input = r#"
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __generator = (this && this.__generator) || function (thisArg, body) {
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g = Object.create((typeof Iterator === "function" ? Iterator : Object).prototype);
    return g.next = verb(0), g["throw"] = verb(1), g["return"] = verb(2), typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
    function verb(n) { return function (v) { return step([n, v]); }; }
    function step(op) {
        if (f) throw new TypeError("Generator is already executing.");
        while (g && (g = 0, op[0] && (_ = 0)), _) try {
            if (f = 1, y && (t = op[0] & 2 ? y["return"] : op[0] ? y["throw"] || ((t = y["return"]) && t.call(y), 0) : y.next) && !(t = t.call(y, op[1])).done) return t;
            if (y = 0, t) op = [op[0] & 2, t.value];
            switch (op[0]) {
                case 0: case 1: t = op; break;
                case 4: _.label++; return { value: op[1], done: false };
                case 5: _.label++; y = op[1]; op = [0]; continue;
                case 7: op = _.ops.pop(); _.trys.pop(); continue;
                default:
                    if (!(t = _.trys, t = t.length > 0 && t[t.length - 1]) && (op[0] === 6 || op[0] === 2)) { _ = 0; continue; }
                    if (op[0] === 3 && (!t || (op[1] > t[0] && op[1] < t[3]))) { _.label = op[1]; break; }
                    if (op[0] === 6 && _.label < t[1]) { _.label = t[1]; t = op; break; }
                    if (t && _.label < t[2]) { _.label = t[2]; _.ops.push(op); break; }
                    if (t[2]) _.ops.pop();
                    _.trys.pop(); continue;
            }
            op = body.call(thisArg, _);
        } catch (e) { op = [6, e]; y = 0; } finally { f = t = 0; }
        if (op[0] & 5) throw op[1]; return { value: op[0] ? op[1] : void 0, done: true };
    }
};
function f(it, check) {
    return __awaiter(this, void 0, void 0, function () {
        var r;
        return __generator(this, function (_a) {
            switch (_a.label) {
                case 0:
                    _a.trys.push([0, , 6, 8]);
                    _a.label = 1;
                case 1: return [4 /*yield*/, it.next()];
                case 2:
                    r = _a.sent();
                    return [4 /*yield*/, check(r)];
                case 3:
                    if (_a.sent())
                        return [2 /*return*/, r];
                    _a.label = 4;
                case 4: return [3 /*break*/, 1];
                case 5: return [3 /*break*/, 8];
                case 6: return [4 /*yield*/, close(it)];
                case 7:
                    _a.sent();
                    return [7 /*endfinally*/];
                case 8: return [2 /*return*/];
            }
        });
    });
}
"#;
    let expected = r#"
async function f(it, check) {
  let r;
  try {
    for (;;) {
      r = await it.next();
      if (await check(r)) {
        return r;
      }
    }
  } finally {
    await close(it);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn ts_es5_indexed_loop_with_early_return_inside_try_finally() {
    let input = r#"
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __generator = (this && this.__generator) || function (thisArg, body) {
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g = Object.create((typeof Iterator === "function" ? Iterator : Object).prototype);
    return g.next = verb(0), g["throw"] = verb(1), g["return"] = verb(2), typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
    function verb(n) { return function (v) { return step([n, v]); }; }
    function step(op) {
        if (f) throw new TypeError("Generator is already executing.");
        while (g && (g = 0, op[0] && (_ = 0)), _) try {
            if (f = 1, y && (t = op[0] & 2 ? y["return"] : op[0] ? y["throw"] || ((t = y["return"]) && t.call(y), 0) : y.next) && !(t = t.call(y, op[1])).done) return t;
            if (y = 0, t) op = [op[0] & 2, t.value];
            switch (op[0]) {
                case 0: case 1: t = op; break;
                case 4: _.label++; return { value: op[1], done: false };
                case 5: _.label++; y = op[1]; op = [0]; continue;
                case 7: op = _.ops.pop(); _.trys.pop(); continue;
                default:
                    if (!(t = _.trys, t = t.length > 0 && t[t.length - 1]) && (op[0] === 6 || op[0] === 2)) { _ = 0; continue; }
                    if (op[0] === 3 && (!t || (op[1] > t[0] && op[1] < t[3]))) { _.label = op[1]; break; }
                    if (op[0] === 6 && _.label < t[1]) { _.label = t[1]; t = op; break; }
                    if (t && _.label < t[2]) { _.label = t[2]; _.ops.push(op); break; }
                    if (t[2]) _.ops.pop();
                    _.trys.pop(); continue;
            }
            op = body.call(thisArg, _);
        } catch (e) { op = [6, e]; y = 0; } finally { f = t = 0; }
        if (op[0] & 5) throw op[1]; return { value: op[0] ? op[1] : void 0, done: true };
    }
};
function f(items, check) {
    return __awaiter(this, void 0, void 0, function () {
        var _i, items_1, r;
        return __generator(this, function (_a) {
            switch (_a.label) {
                case 0:
                    _a.trys.push([0, , 5, 7]);
                    _i = 0, items_1 = items;
                    _a.label = 1;
                case 1:
                    if (!(_i < items_1.length)) return [3 /*break*/, 4];
                    r = items_1[_i];
                    return [4 /*yield*/, check(r)];
                case 2:
                    if (_a.sent())
                        return [2 /*return*/, r];
                    _a.label = 3;
                case 3:
                    _i++;
                    return [3 /*break*/, 1];
                case 4: return [3 /*break*/, 7];
                case 5: return [4 /*yield*/, close(items)];
                case 6:
                    _a.sent();
                    return [7 /*endfinally*/];
                case 7: return [2 /*return*/, null];
            }
        });
    });
}
"#;
    let expected = r#"
async function f(items, check) {
  let _i;
  let items_1;
  let r;
  try {
    _i = 0;
    items_1 = items;
    for (; _i < items_1.length; _i++) {
      r = items_1[_i];
      if (await check(r)) {
        return r;
      }
    }
  } finally {
    await close(items);
  }
  return null;
}
"#;
    assert_eq_normalized(&render(input), expected);
}
