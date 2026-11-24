mod common;

use common::{assert_eq_normalized, render, render_pipeline_between, render_rule};
use wakaru_core::facts::{HelperExportFact, HelperKind, ModuleFacts, ModuleFactsMap};
use wakaru_core::rules::UnRegenerator;

fn apply(input: &str) -> String {
    render_rule(input, UnRegenerator::new)
}

fn apply_with_helper_facts(input: &str) -> String {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "./module-async.js",
        ModuleFacts {
            helper_exports: vec![HelperExportFact {
                exported: "default".into(),
                local: Some("asyncToGenerator".into()),
                kind: HelperKind::AsyncToGenerator,
            }],
            ..Default::default()
        },
    );
    facts.insert(
        "./module-runtime.js",
        ModuleFacts {
            helper_exports: vec![HelperExportFact {
                exported: "default".into(),
                local: Some("runtime".into()),
                kind: HelperKind::RegeneratorRuntime,
            }],
            ..Default::default()
        },
    );

    render_rule(input, |mark| UnRegenerator::new_with_facts(mark, &facts))
}

// ── Pure generators (regeneratorRuntime.wrap → function*) ───────────────────

#[test]
fn simple_generator_single_yield() {
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen() {
  return regeneratorRuntime.wrap(function(_context) {
    while (true) {
      switch (_context.prev = _context.next) {
        case 0:
          _context.next = 2;
          return someValue;
        case 2:
        case "end":
          return _context.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen() {
  yield someValue;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_multiple_yields() {
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen() {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          e.next = 2;
          return 1;
        case 2:
          e.next = 4;
          return 2;
        case 4:
          e.next = 6;
          return 3;
        case 6:
        case "end":
          return e.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen() {
  yield 1;
  yield 2;
  yield 3;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_with_return_value() {
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen() {
  return regeneratorRuntime.wrap(function(_context) {
    while (true) {
      switch (_context.prev = _context.next) {
        case 0:
          _context.next = 2;
          return fetchData();
        case 2:
          return _context.abrupt("return", _context.sent);
        case 3:
        case "end":
          return _context.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen() {
  return yield fetchData();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_preserves_hoisted_locals_declared_after_state_loop() {
    let input = r#"
var _marked = regeneratorRuntime.mark(loadValue);
function loadValue() {
  return regeneratorRuntime.wrap(function(_context) {
    while (true) {
      switch (_context.prev = _context.next) {
        case 0:
          localValue = createValue();
          return _context.abrupt("return", localValue);
        case 2:
        case "end":
          return _context.stop();
      }
    }
    var localValue;
  }, _marked, this);
}
"#;
    let expected = r#"
function* loadValue() {
  var localValue;
  localValue = createValue();
  return localValue;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);

    let pipeline_output = render_pipeline_between(input, "UnRegenerator", "VarDeclToLetConst");
    assert!(
        pipeline_output.contains("let localValue;"),
        "the recovered local is written and must not become const:\n{pipeline_output}"
    );
    assert!(
        !pipeline_output.contains("const localValue"),
        "the recovered local must not be misclassified as immutable:\n{pipeline_output}"
    );
}

#[test]
fn generator_with_colliding_callback_local_is_left_unchanged() {
    let input = r#"
var _marked = regeneratorRuntime.mark(loadValue);
function loadValue(localValue) {
  return regeneratorRuntime.wrap(function(_context) {
    while (true) {
      switch (_context.prev = _context.next) {
        case 0:
          localValue = createValue();
          return _context.abrupt("return", localValue);
        case 2:
        case "end":
          return _context.stop();
      }
    }
    var localValue;
  }, _marked, this);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("regeneratorRuntime.wrap"),
        "a callback local that collides with the destination function scope must fail closed:\n{output}"
    );
}

#[test]
fn generator_with_yield_assignment() {
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen() {
  return regeneratorRuntime.wrap(function(_context) {
    while (true) {
      switch (_context.prev = _context.next) {
        case 0:
          _context.next = 2;
          return fetchData();
        case 2:
          result = _context.sent;
          console.log(result);
          return _context.abrupt("return", result);
        case 5:
        case "end":
          return _context.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen() {
  result = yield fetchData();
  console.log(result);
  return result;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_infinite_loop() {
    // Redux-saga style: infinite loop generator
    let input = r#"
var _marked = regeneratorRuntime.mark(watchFetch);
function watchFetch() {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          e.next = 2;
          return take(FETCH_DATA);
        case 2:
          e.next = 4;
          return put(startFetching());
        case 4:
          e.next = 6;
          return put(fetchSuccess([]));
        case 6:
          e.next = 0;
          break;
        case 8:
        case "end":
          return e.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* watchFetch() {
  while (true) {
    yield take(FETCH_DATA);
    yield put(startFetching());
    yield put(fetchSuccess([]));
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_with_statements_before_yield() {
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(url) {
  return regeneratorRuntime.wrap(function(_context) {
    while (true) {
      switch (_context.prev = _context.next) {
        case 0:
          console.log("fetching");
          _context.next = 3;
          return fetch(url);
        case 3:
        case "end":
          return _context.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen(url) {
  console.log("fetching");
  yield fetch(url);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_minified_names() {
    // Minified: state param is 'e', mark var is 'a'
    let input = r#"
var a = regeneratorRuntime.mark(l);
function l() {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          e.next = 2;
          return take(FETCH);
        case 2:
        case "end":
          return e.stop();
      }
    }
  }, a, this);
}
"#;
    let expected = r#"
function* l() {
  yield take(FETCH);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_comma_operator_yield() {
    // Some minifiers merge _context.next = N and return value into: return e.next = N, value
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen() {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          return e.next = 2, fetchData();
        case 2:
          return e.next = 4, processData();
        case 4:
        case "end":
          return e.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen() {
  yield fetchData();
  yield processData();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_comma_operator_yield_keeps_middle_expressions() {
    // The pipeline splits this sequence before UnRegenerator runs, so only the
    // rule on its own sees it: `before()` and `during()` run before the
    // yield, in order.
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen() {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          return e.next = 2, before(), during(), first();
        case 2:
          x = e.sent;
          return e.next = 4, second(x);
        case 4:
        case "end":
          return e.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen() {
  before();
  during();
  x = yield first();
  yield second(x);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn multiple_generators_in_module() {
    let input = r#"
var a = regeneratorRuntime.mark(gen1);
var b = regeneratorRuntime.mark(gen2);
function gen1() {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          e.next = 2;
          return 1;
        case 2:
        case "end":
          return e.stop();
      }
    }
  }, a, this);
}
function gen2() {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          e.next = 2;
          return 2;
        case 2:
        case "end":
          return e.stop();
      }
    }
  }, b, this);
}
"#;
    let expected = r#"
function* gen1() {
  yield 1;
}
function* gen2() {
  yield 2;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_try_finally_drops_finish() {
    let input = r#"
var _marked = regeneratorRuntime.mark(g);
function g() {
  return regeneratorRuntime.wrap(function(_ctx) {
    while (true) {
      switch (_ctx.prev = _ctx.next) {
        case 0:
          _ctx.prev = 0;
          _ctx.next = 3;
          return doThing();
        case 3:
          return _ctx.finish(0);
        case 5:
          _ctx.prev = 5;
          cleanup();
          return _ctx.finish(5);
        case 8:
        case "end":
          return _ctx.stop();
      }
    }
  }, _marked, null, [[0, , 5]]);
}
"#;
    let output = apply(input);
    assert!(
        !output.contains("_ctx"),
        "should not leak state object, got:\n{output}"
    );
    assert!(
        output.contains("finally"),
        "should reconstruct finally block, got:\n{output}"
    );
    assert!(
        output.contains("yield doThing()"),
        "should keep yielded try body, got:\n{output}"
    );
    assert!(
        output.contains("cleanup()"),
        "should keep finalizer body, got:\n{output}"
    );
}

#[test]
fn generator_try_finally_drops_short_finish() {
    let input = r#"
var _marked = regeneratorRuntime.mark(g);
function g() {
  return regeneratorRuntime.wrap(function(_ctx) {
    while (true) {
      switch (_ctx.prev = _ctx.next) {
        case 0:
          _ctx.prev = 0;
          _ctx.next = 3;
          return doThing();
        case 3:
          return _ctx.f(0);
        case 5:
          _ctx.prev = 5;
          cleanup();
          return _ctx.f(5);
        case 8:
        case "end":
          return _ctx.stop();
      }
    }
  }, _marked, null, [[0, , 5]]);
}
"#;
    let output = apply(input);
    assert!(
        !output.contains("_ctx"),
        "should not leak state object, got:\n{output}"
    );
    assert!(
        output.contains("finally"),
        "should reconstruct finally block, got:\n{output}"
    );
    assert!(
        output.contains("yield doThing()"),
        "should keep yielded try body, got:\n{output}"
    );
    assert!(
        output.contains("cleanup()"),
        "should keep finalizer body, got:\n{output}"
    );
}

#[test]
fn generator_try_catch_without_region_arg_bails_conservatively() {
    let input = r#"
var _marked = regeneratorRuntime.mark(g);
function g() {
  return regeneratorRuntime.wrap(function(_ctx) {
    while (true) {
      switch (_ctx.prev = _ctx.next) {
        case 0:
          _ctx.prev = 0;
          _ctx.next = 3;
          return doThing();
        case 3:
          _ctx.next = 8;
          break;
        case 5:
          _ctx.prev = 5;
          _ctx.t0 = _ctx.catch(0);
          handle(_ctx.t0);
        case 8:
        case "end":
          return _ctx.stop();
      }
    }
  }, _marked);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("regeneratorRuntime.wrap"),
        "should leave catch state machine without try-region metadata unchanged, got:\n{output}"
    );
    assert!(
        output.contains("_ctx.catch(0)"),
        "should preserve catch call without inferring a try region, got:\n{output}"
    );
}

#[test]
fn generator_try_catch_with_region_arg_still_works() {
    let input = r#"
var _marked = regeneratorRuntime.mark(g);
function g() {
  return regeneratorRuntime.wrap(function(_ctx) {
    while (true) {
      switch (_ctx.prev = _ctx.next) {
        case 0:
          _ctx.prev = 0;
          _ctx.next = 3;
          return doThing();
        case 3:
          _ctx.next = 8;
          break;
        case 5:
          _ctx.prev = 5;
          _ctx.t0 = _ctx.catch(0);
          handle(_ctx.t0);
        case 8:
        case "end":
          return _ctx.stop();
      }
    }
  }, _marked, null, [[0, 5]]);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("function* g()"),
        "should convert to generator, got:\n{output}"
    );
    assert!(
        output.contains("try"),
        "should reconstruct try block, got:\n{output}"
    );
    assert!(
        output.contains("catch (error)"),
        "should reconstruct catch binding, got:\n{output}"
    );
    assert!(
        output.contains("yield doThing()"),
        "should keep yielded try body, got:\n{output}"
    );
    assert!(
        output.contains("handle(error)"),
        "should replace catch alias, got:\n{output}"
    );
    assert!(
        !output.contains("_ctx"),
        "should not leak state object, got:\n{output}"
    );
}

#[test]
fn generator_delegate_yield_restored() {
    let input = r#"
var _marked = regeneratorRuntime.mark(read_all);
function read_all(source) {
  return regeneratorRuntime.wrap(function read_all$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        _context.prev = 0;
        _context.next = 3;
        return start_read(source);
      case 3:
        return _context.delegateYield(read_chunks(source), "t0", 4);
      case 4:
        _context.next = 6;
        return finish_read(source);
      case 6:
        return _context.abrupt("return", _context.sent);
      case 7:
        _context.prev = 7;
        _context.next = 10;
        return close_reader(source);
      case 10:
        return _context.finish(7);
      case 11:
      case "end":
        return _context.stop();
    }
  }, _marked, null, [[0,, 7, 11]]);
}
"#;
    let expected = r#"
function* read_all(source) {
  try {
    yield start_read(source);
    yield* read_chunks(source);
    return yield finish_read(source);
  } finally {
    yield close_reader(source);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_recovers_conditional_yield_default_via_forward_jump() {
    // Babel 7.28+ `_regenerator().w` lowers `x == null ? yield f() : x` into a
    // forward conditional jump: `if (!(x == null)) { _context.n = 2; break; }`
    // selecting between the yield branch and the fallthrough. The decoder must
    // structure that jump back into a ternary assignment. The `_t` join temp
    // then folds into its single read like an await-spill temp.
    let input = r#"
var _marked = _regenerator().m(pick);
function pick(input) {
  var source, _t;
  return _regenerator().w(function (_context) {
    while (1) switch (_context.n) {
      case 0:
        if (!(input == null)) {
          _context.n = 2;
          break;
        }
        _context.n = 1;
        return load_user();
      case 1:
        _t = _context.v;
        _context.n = 3;
        break;
      case 2:
        _t = input;
      case 3:
        source = _t;
        return _context.a(2, source);
    }
  }, _marked);
}
"#;
    let expected = r#"
function* pick(input) {
  var source;
  source = !(input == null) ? input : yield load_user();
  return source;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn generator_recovers_forward_if_else_branch() {
    // A forward jump to an else label followed by an explicit jump to a join
    // label is structured as a normal if/else instead of forcing rollback.
    let input = r#"
var _marked = _regenerator().m(pick);
function pick(input) {
  return _regenerator().w(function (_context) {
    while (1) switch (_context.n) {
      case 0:
        if (!input) {
          _context.n = 2;
          break;
        }
        sideEffect();
        _context.n = 3;
        break;
      case 2:
        other();
      case 3:
        return _context.a(2);
    }
  }, _marked);
}
"#;
    let expected = r#"
function* pick(input) {
  if (input) {
    sideEffect();
  } else {
    other();
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn generator_short_delegate_yield_restored_with_minified_values_helper() {
    // Top-level mangling renames `_regeneratorValues` to a short alias. The
    // delegate-yield wrapper must still be stripped (matched by body shape) and
    // the now-dead helper removed.
    let input = r#"
function v(o) {
  if (o != null) {
    var s = o["function" == typeof Symbol && Symbol.iterator || "@@iterator"], n = 0;
    if (s) return s.call(o);
    if ("function" == typeof o.next) return o;
    if (!isNaN(o.length)) return {
      next: function () {
        return o && n >= o.length && (o = void 0), { value: o && o[n++], done: !o };
      }
    };
  }
  throw new TypeError(typeof o + " is not iterable");
}
var _marked = _regenerator().m(read_all);
function read_all(source) {
  return _regenerator().w(function(_context) {
    while (1) switch (_context.p = _context.n) {
      case 0:
        _context.p = 0;
        _context.n = 1;
        return start_read(source);
      case 1:
        return _context.d(v(read_chunks(source)), 2);
      case 2:
        _context.n = 3;
        return finish_read(source);
      case 3:
        return _context.a(2, _context.v);
      case 4:
        _context.p = 4;
        _context.n = 5;
        return close_reader(source);
      case 5:
        return _context.f(4);
      case 6:
        return _context.a(2);
    }
  }, _marked, null, [[0,, 4, 6]]);
}
"#;
    let expected = r#"
function* read_all(source) {
  try {
    yield start_read(source);
    yield* read_chunks(source);
    return yield finish_read(source);
  } finally {
    yield close_reader(source);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_delegate_yield_result_is_restored() {
    let input = r#"
var _marked = regeneratorRuntime.mark(read_all);
function read_all(source) {
  var result;
  return regeneratorRuntime.wrap(function read_all$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        return _context.delegateYield(read_chunks(source), "t0", 1);
      case 1:
        result = _context.t0;
        return _context.abrupt("return", result);
      case 2:
      case "end":
        return _context.stop();
    }
  }, _marked);
}
"#;
    let expected = r#"
function* read_all(source) {
  var result;
  result = yield* read_chunks(source);
  return result;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_short_delegate_yield_restored() {
    let input = r#"
var _marked = _regenerator().m(read_all);
function read_all(source) {
  return _regenerator().w(function(_context) {
    while (1) switch (_context.p = _context.n) {
      case 0:
        _context.p = 0;
        _context.n = 1;
        return start_read(source);
      case 1:
        return _context.d(_regeneratorValues(read_chunks(source)), 2);
      case 2:
        _context.n = 3;
        return finish_read(source);
      case 3:
        return _context.a(2, _context.v);
      case 4:
        _context.p = 4;
        _context.n = 5;
        return close_reader(source);
      case 5:
        return _context.f(4);
      case 6:
        return _context.a(2);
    }
  }, _marked, null, [[0,, 4, 6]]);
}
"#;
    let expected = r#"
function* read_all(source) {
  try {
    yield start_read(source);
    yield* read_chunks(source);
    return yield finish_read(source);
  } finally {
    yield close_reader(source);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// ── Terser-wrapped callbacks ────────────────────────────────────────────────
//
// Terser before 5.43 wraps every function passed as a call argument in
// parentheses (its default `wrap_func_args` output option). The state machine
// callback then reads `_regenerator().w((function (_context) { … }), _marked)`.

#[test]
fn compact_generator_with_parenthesized_callback_is_restored() {
    // producer babel@7.29 preset-env (ie 11) + terser@5.31 --module
    let input = r#"
var _marked = _regenerator().m(count);
function count(limit) {
  var i;
  return _regenerator().w((function (_context) {
    while (1) switch (_context.n) {
      case 0:
        i = 0;
      case 1:
        if (!(i < limit)) { _context.n = 3; break; }
        _context.n = 2;
        return i;
      case 2:
        i++;
        _context.n = 1;
        break;
      case 3:
        return _context.a(2, "done");
    }
  }), _marked);
}
export { count };
"#;
    let expected = r#"
function* count(limit) {
  let i = 0;
  for (; i < limit; i++) {
    yield i;
  }
  return "done";
}
export { count };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn compact_async_function_with_parenthesized_callbacks_is_restored() {
    // producer babel@7.29 preset-env (ie 11) + terser@5.31 --module
    let input = r#"
function asyncGeneratorStep(n, t, e, r, o, a, c) {
  try { var i = n[a](c), u = i.value; } catch (n) { return void e(n); }
  i.done ? t(u) : Promise.resolve(u).then(r, o);
}
function _asyncToGenerator(n) {
  return function () {
    var t = this, e = arguments;
    return new Promise((function (r, o) {
      var a = n.apply(t, e);
      function _next(n) { asyncGeneratorStep(a, r, o, _next, _throw, "next", n); }
      function _throw(n) { asyncGeneratorStep(a, r, o, _next, _throw, "throw", n); }
      _next(void 0);
    }));
  };
}
export function load(_x) {
  return _load.apply(this, arguments);
}
function _load() {
  _load = _asyncToGenerator(_regenerator().m((function _callee(url) {
    var response;
    return _regenerator().w((function (_context2) {
      while (1) switch (_context2.n) {
        case 0:
          _context2.n = 1;
          return fetch(url);
        case 1:
          response = _context2.v;
          if (response.ok) { _context2.n = 2; break; }
          throw new Error("bad");
        case 2:
          return _context2.a(2, response.json());
      }
    }), _callee);
  })));
  return _load.apply(this, arguments);
}
"#;
    let expected = r#"
export async function load(url) {
  let response;
  response = await fetch(url);
  if (!response.ok) {
    throw new Error("bad");
  }
  return response.json();
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn legacy_generator_with_parenthesized_callback_is_restored() {
    // producer babel@7.23 preset-env (ie 11) + terser@5.31 --module
    let input = r#"
var _marked = _regeneratorRuntime().mark(count);
function count(limit) {
  var i;
  return _regeneratorRuntime().wrap((function count$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        i = 0;
      case 1:
        if (!(i < limit)) { _context.next = 7; break; }
        _context.next = 4;
        return i;
      case 4:
        i++;
        _context.next = 1;
        break;
      case 7:
        return _context.abrupt("return", "done");
      case 8:
      case "end":
        return _context.stop();
    }
  }), _marked);
}
export { count };
"#;
    let expected = r#"
function* count(limit) {
  let i = 0;
  for (; i < limit; i++) {
    yield i;
  }
  return "done";
}
export { count };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn legacy_async_function_with_parenthesized_callbacks_is_restored() {
    // producer babel@7.23 preset-env (ie 11) + terser@5.31 --module
    let input = r#"
function asyncGeneratorStep(gen, resolve, reject, _next, _throw, key, arg) {
  try { var info = gen[key](arg); var value = info.value; } catch (error) { reject(error); return; }
  if (info.done) { resolve(value); } else { Promise.resolve(value).then(_next, _throw); }
}
function _asyncToGenerator(fn) {
  return function () {
    var self = this, args = arguments;
    return new Promise((function (resolve, reject) {
      var gen = fn.apply(self, args);
      function _next(value) { asyncGeneratorStep(gen, resolve, reject, _next, _throw, "next", value); }
      function _throw(err) { asyncGeneratorStep(gen, resolve, reject, _next, _throw, "throw", err); }
      _next(void 0);
    }));
  };
}
export function load(_x) {
  return _load.apply(this, arguments);
}
function _load() {
  _load = _asyncToGenerator(_regeneratorRuntime().mark((function _callee(url) {
    var response;
    return _regeneratorRuntime().wrap((function _callee$(_context2) {
      while (1) switch (_context2.prev = _context2.next) {
        case 0:
          _context2.next = 2;
          return fetch(url);
        case 2:
          response = _context2.sent;
          if (response.ok) { _context2.next = 5; break; }
          throw new Error("bad");
        case 5:
          return _context2.abrupt("return", response.json());
        case 6:
        case "end":
          return _context2.stop();
      }
    }), _callee);
  })));
  return _load.apply(this, arguments);
}
"#;
    let expected = r#"
export async function load(url) {
  let response;
  response = await fetch(url);
  if (!response.ok) {
    throw new Error("bad");
  }
  return response.json();
}
"#;
    assert_eq_normalized(&render(input), expected);
}

// ── _asyncToGenerator (Babel async functions) ───────────────────────────────

#[test]
fn async_to_generator_with_native_generator() {
    // Babel with native generator support: _asyncToGenerator(function*() { ... })()
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function myAsync() {
  return _asyncToGenerator(function*() {
    yield fetch("/api");
    yield process();
  })();
}
"#;
    let expected = r#"
async function myAsync() {
  await fetch("/api");
  await process();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_with_regenerator() {
    // Full Babel: _asyncToGenerator + regeneratorRuntime
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function myAsync() {
  return _asyncToGenerator(regeneratorRuntime.mark(function _callee() {
    return regeneratorRuntime.wrap(function(_context) {
      while (true) {
        switch (_context.prev = _context.next) {
          case 0:
            _context.next = 2;
            return fetch("/api");
          case 2:
            _context.next = 4;
            return process();
          case 4:
          case "end":
            return _context.stop();
        }
      }
    }, _callee, this);
  }))();
}
"#;
    let expected = r#"
async function myAsync() {
  await fetch("/api");
  await process();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_with_return_value() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function fetchUser(id) {
  return _asyncToGenerator(function*() {
    var response = yield fetch("/api/users/" + id);
    var data = yield response.json();
    return data;
  })();
}
"#;
    let expected = r#"
async function fetchUser(id) {
  var response = await fetch("/api/users/" + id);
  var data = await response.json();
  return data;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn swc_async_to_generator_with_ts_generator() {
    let input = r#"
function _async_to_generator(fn) {
  return function() {
    var self = this, args = arguments;
    return new Promise(function(resolve, reject) {
      var gen = fn.apply(self, args);
      function _next(value) {
        resolve(gen.next(value).value);
      }
      _next(undefined);
    });
  };
}
function _ts_generator(thisArg, body) {
  var t, _ = {
    label: 0,
    sent: function() { return t[1]; },
    trys: [],
    ops: []
  };
}
function load_user(app_id) {
  return _async_to_generator(function() {
    var response, data;
    return _ts_generator(this, function(_state) {
      switch (_state.label) {
        case 0:
          return [4, fetch_user(app_id)];
        case 1:
          response = _state.sent();
          return [4, response.json()];
        case 2:
          data = _state.sent();
          return [2, data];
      }
    });
  })();
}
"#;
    let output = apply(input);
    assert!(
        output.contains("async function load_user(app_id)"),
        "should restore SWC async wrapper, got:\n{output}"
    );
    assert!(
        output.contains("response = await fetch_user(app_id)")
            && output.contains("data = await response.json()")
            && output.contains("return data"),
        "should restore awaited SWC state-machine body, got:\n{output}"
    );
}

#[test]
fn swc_external_async_to_generator_import() {
    let input = r#"
import { _ as _async_to_generator } from "@swc/helpers/_/_async_to_generator";
function myAsync() {
    return _async_to_generator(function*() {
        yield fetch("/api");
        yield process();
    })();
}
"#;
    let expected = r#"
async function myAsync() {
    await fetch("/api");
    await process();
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn swc_nested_async_callback_keeps_inner_sent_scoped() {
    let input = r#"
function _async_to_generator(fn) {
  return function() {
    var self = this, args = arguments;
    return new Promise(function(resolve, reject) {
      var gen = fn.apply(self, args);
      function _next(value) {
        resolve(gen.next(value).value);
      }
      _next(undefined);
    });
  };
}
function _ts_generator(thisArg, body) {
  var t, _ = {
    label: 0,
    sent: function() { return t[1]; },
    trys: [],
    ops: []
  };
}
use(function run_pipeline(source) {
  return _async_to_generator(function() {
    var steps;
    return _ts_generator(this, function(_state) {
      switch (_state.label) {
        case 0:
          return [4, load_steps(source)];
        case 1:
          return [2, (steps = _state.sent()).map(function(step) {
            return _async_to_generator(function() {
              return _ts_generator(this, function(_state) {
                switch (_state.label) {
                  case 0:
                    return [4, step.run(source)];
                  case 1:
                    return [2, _state.sent()];
                }
              });
            })();
          })];
      }
    });
  })();
});
"#;
    let output = apply(input);
    assert!(
        output.contains("use(async function run_pipeline(source)"),
        "should restore outer SWC async wrapper, got:\n{output}"
    );
    assert!(
        output.contains("return (steps = await load_steps(source)).map(async function(step)"),
        "should keep the outer await result scoped to the outer state machine, got:\n{output}"
    );
    assert!(
        output.contains("return await step.run(source)"),
        "should preserve the nested callback return value, got:\n{output}"
    );
    assert!(
        !output.contains("return await load_steps(source)"),
        "outer sent replacement must not leak into nested callback, got:\n{output}"
    );
}

#[test]
fn esbuild_async_arrow_helper() {
    let input = r#"
var __async = (__this, __arguments, generator) => {
  return new Promise((resolve, reject) => {
    var step = (x) => x.done ? resolve(x.value) : Promise.resolve(x.value).then(fulfilled, rejected);
    step((generator = generator.apply(__this, __arguments)).next());
  });
};
const load_user = (app_id) => __async(null, null, function* () {
  return yield fetch_user(app_id);
});
use(load_user);
"#;
    let expected = r#"
const load_user = async (app_id) => {
  return await fetch_user(app_id);
};
use(load_user);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn esbuild_async_helper_with_deconflicted_name() {
    // A bundle holding several `__async` copies renames all but one:
    // esbuild to `__async2`, rollup to `__async$1`.
    let input = r#"
var __async$1 = (__this, __arguments, generator) => new Promise((resolve) => {
  step((generator = generator.apply(__this, __arguments)).next());
});
function load_user(app_id) {
  return __async$1(this, arguments, function* () {
    var response = yield fetch_user(app_id);
    return response;
  });
}
var __async2 = (__this, __arguments, generator) => new Promise((resolve) => {
  step((generator = generator.apply(__this, __arguments)).next());
});
function load_team(team_id) {
  return __async2(this, arguments, function* () {
    return yield fetch_team(team_id);
  });
}
var __asyncX = (__this, __arguments, generator) => new Promise((resolve) => {
  step((generator = generator.apply(__this, __arguments)).next());
});
function other(app_id) {
  return __asyncX(this, arguments, function* () {
    return yield fetch_user(app_id);
  });
}
"#;
    let expected = r#"
async function load_user(app_id) {
  var response = await fetch_user(app_id);
  return response;
}
async function load_team(team_id) {
  return await fetch_team(team_id);
}
var __asyncX = (__this, __arguments, generator) => new Promise((resolve) => {
  step((generator = generator.apply(__this, __arguments)).next());
});
function other(app_id) {
  return __asyncX(this, arguments, function* () {
    return yield fetch_user(app_id);
  });
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn esbuild_async_function_helper() {
    let input = r#"
var __async = (__this, __arguments, generator) => new Promise((resolve) => {
  step((generator = generator.apply(__this, __arguments)).next());
});
function load_user(app_id) {
  return __async(this, arguments, function* () {
    var response = yield fetch_user(app_id);
    return response;
  });
}
"#;
    let expected = r#"
async function load_user(app_id) {
  var response = await fetch_user(app_id);
  return response;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn esbuild_mangled_async_helper_recovers_loop_try_catch() {
    let input = r#"
var e = (e, t, n) => new Promise((r, c) => {
  var i = e => { try { o(n.next(e)); } catch (e) { c(e); } };
  var l = e => { try { o(n.throw(e)); } catch (e) { c(e); } };
  var o = e => e.done ? r(e.value) : Promise.resolve(e.value).then(i, l);
  o((n = n.apply(e, t)).next());
});
function collect_enabled(items) {
  return e(this, null, function*() {
    const output = [];
    for (let index = 0; index < items.length; index++) {
      const item = items[index];
      if (item.enabled) {
        try {
          output.push(yield fetch_item(item.id));
        } catch (error) {
          output.push(yield recover_item(item, error));
        }
      }
    }
    return output;
  });
}
"#;
    let expected = r#"
async function collect_enabled(items) {
  const output = [];
  for (let index = 0; index < items.length; index++) {
    const item = items[index];
    if (item.enabled) {
      try {
        output.push(await fetch_item(item.id));
      } catch (error) {
        output.push(await recover_item(item, error));
      }
    }
  }
  return output;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn esbuild_mangled_async_generator_helper_is_not_async() {
    // `__asyncGenerator` also calls `.apply` and builds promises, but returns
    // an iterator. Reading it as `__async` would turn the generator into an
    // async function and its `yield`s into `await`s.
    let input = r#"
var s = function(a, b) { this[0] = a, this[1] = b; };
var B = (a, b, c) => {
  var d = (f, h, i, k) => {
    try {
      var n = c[f](h), o = (h = n.value) instanceof s, t = n.done;
      Promise.resolve(o ? h[0] : h).then(j => o ? d(f === "return" ? f : "next", h[1] ? {done: j.done, value: j.value} : j, i, k) : i({value: j, done: t})).catch(j => d("throw", j, i, k));
    } catch (j) { k(j); }
  }, g = f => e[f] = h => new Promise((i, k) => d(f, h, i, k)), e = {};
  return c = c.apply(a, b), e[Symbol.asyncIterator] = () => e, g("next"), g("throw"), g("return"), e;
};
function items(source) {
  return B(this, null, function* () {
    yield source.first;
    yield new s(source.next());
  });
}
use(items);
"#;
    let output = apply(input);
    assert!(
        !output.contains("async function items"),
        "an async generator must not become an async function, got:\n{output}"
    );
    assert!(
        output.contains("yield source.first"),
        "the generator body must keep its yields, got:\n{output}"
    );
}

#[test]
fn esbuild_yield_star_helper_is_unwrapped() {
    let input = r#"
var __knownSymbol = (name, symbol) => (symbol = Symbol[name]) ? symbol : Symbol.for("Symbol." + name);
var __await = function(promise, isYieldStar) {
  this[0] = promise;
  this[1] = isYieldStar;
};
var __yieldStar = (value) => {
  var obj = value[__knownSymbol("asyncIterator")], isAwait = false, method, it = {};
  return obj == null
    ? (obj = value[__knownSymbol("iterator")](), method = (k) => it[k] = (x) => obj[k](x))
    : (method = (k) => it[k] = (v) => ({ done: false, value: new __await(v, 1) })),
    it;
};
function* read_all(source) {
  yield* __yieldStar(read_chunks(source));
}
"#;
    let output = apply(input);
    assert!(
        output.contains("yield* read_chunks(source)"),
        "should unwrap esbuild yield-star helper, got:\n{output}"
    );
    assert!(
        !output.contains("__yieldStar(read_chunks"),
        "rewritten delegate yield should not keep the esbuild helper call, got:\n{output}"
    );
}

#[test]
fn esbuild_async_helper_ignores_shadowed_promise() {
    let input = r#"
const Promise = makePromise();
var __async = (__this, __arguments, generator) => new Promise((resolve) => {
  step((generator = generator.apply(__this, __arguments)).next());
});
function load_user(app_id) {
  return __async(this, arguments, function* () {
    return yield fetch_user(app_id);
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("var __async"),
        "shadowed Promise helper should not be classified as esbuild __async, got:\n{output}"
    );
    assert!(
        !output.contains("async function load_user"),
        "shadowed Promise helper must not be rewritten to native async, got:\n{output}"
    );
}

#[test]
fn esbuild_async_helper_preserves_side_effectful_context_args() {
    let input = r#"
var __async = (__this, __arguments, generator) => new Promise((resolve) => {
  var fulfilled = (value) => step(generator.next(value));
  var step = (x) => x.done ? resolve(x.value) : Promise.resolve(x.value).then(fulfilled);
  step((generator = generator.apply(__this, __arguments)).next());
});
function load_user(app_id) {
  return __async(get_this(), get_args(), function* () {
    const response = yield fetch_user(app_id);
    return response;
  });
}
"#;
    let output = apply(input);
    assert!(
        output.contains("__async(get_this(), get_args()"),
        "side-effectful __async receiver/arguments must be preserved, got:\n{output}"
    );
    assert!(
        output.contains("yield fetch_user(app_id)"),
        "unsafe __async call should keep the generator argument intact, got:\n{output}"
    );
}

#[test]
fn babel_async_arrow_iife_trampoline() {
    let input = r#"
const load_user = function () {
  var _ref = async function _callee(app_id) {
    return await fetch_user(app_id);
  };
  return function load_user(_x) {
    return _ref.apply(this, arguments);
  };
}();
use(load_user);
"#;
    let expected = r#"
const load_user = async function(app_id) {
  return await fetch_user(app_id);
};
use(load_user);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn babel_nested_async_callback_iife_trampoline() {
    let input = r#"
const run_pipeline = async function(source) {
  const steps = await load_steps(source);
  return steps.map(function () {
    var _ref2 = async function _callee(step) {
      return await step.run(source);
    };
    return function (_x2) {
      return _ref2.apply(this, arguments);
    };
  }());
};
use(run_pipeline);
"#;
    let expected = r#"
const run_pipeline = async function(source) {
  const steps = await load_steps(source);
  return steps.map(async function(step) {
    return await step.run(source);
  });
};
use(run_pipeline);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn babel_nested_async_callback_arrow_iife_trampoline() {
    let input = r#"
const run_pipeline = async (source)=>{
  let steps;
  steps = await load_steps(source);
  return steps.map((()=>{
    const _ref2 = async function _callee(step) {
      return await step.run(source);
    };
    return function(_x2) {
      return _ref2.apply(this, arguments);
    };
  })());
};
use(run_pipeline);
"#;
    let expected = r#"
const run_pipeline = async (source)=>{
  let steps;
  steps = await load_steps(source);
  return steps.map(async function(step) {
    return await step.run(source);
  });
};
use(run_pipeline);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn babel_async_arrow_forwarding_iife_trampoline() {
    let input = r#"
const load_user = (() => {
  const _load_user = async function _callee(app_id) {
    return await fetch_user(app_id);
  };
  function load_user(_x) {
    return _load_user.apply(this, arguments);
  }
  return load_user;
})();
use(load_user);
"#;
    let expected = r#"
const load_user = async function(app_id) {
  return await fetch_user(app_id);
};
use(load_user);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn babel_nested_async_arrow_forwarding_iife_trampoline() {
    let input = r#"
const run_pipeline = (() => {
  const _run_pipeline = async function _callee(source) {
    const steps = await load_steps(source);
    return steps.map((step) => (() => {
      const _callback = async function _callee2() {
        return await step.run(source);
      };
      function callback(_x) {
        return _callback.apply(this, arguments);
      }
      return callback;
    })());
  };
  function run_pipeline(_x) {
    return _run_pipeline.apply(this, arguments);
  }
  return run_pipeline;
})();
use(run_pipeline);
"#;
    let expected = r#"
const run_pipeline = async function(source) {
  const steps = await load_steps(source);
  return steps.map((step) => async function() {
    return await step.run(source);
  });
};
use(run_pipeline);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn babel_async_arrow_sequence_trampoline() {
    let input = r#"
_ref = async function _callee(app_id) {
  return await fetch_user(app_id);
};
const load_user = function load_user(_x) {
  return _ref.apply(this, arguments);
};
var _ref;
use(load_user);
"#;
    let expected = r#"
const load_user = async function(app_id) {
  return await fetch_user(app_id);
};
var _ref;
use(load_user);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn babel_async_arrow_sequence_trampoline_keeps_escaped_private_binding() {
    let input = r#"
_ref = async function _callee(app_id) {
  return await fetch_user(app_id);
};
const load_user = function load_user(_x) {
  return _ref.apply(this, arguments);
};
var _ref;
use(_ref, load_user);
"#;
    let output = apply(input);
    assert!(
        output.contains("_ref = async function"),
        "escaped private async function assignment must be preserved, got:\n{output}"
    );
    assert!(
        output.contains("use(_ref, load_user)"),
        "escaped private binding use must remain valid, got:\n{output}"
    );
}

#[test]
fn babel_729_terser_class_method_sequence_trampoline() {
    // Reproduced from:
    //   @babel/preset-env targeting IE 11 on `class Client { async fetchInternal(request, init) { return await send(request, init); } }`
    //   then Terser compress+mangle. Babel emits the lazy method trampoline;
    //   Terser lowers it to this comma-sequence form.
    let input = r#"
function _asyncToGenerator(e) {
  return function() {
    var r = this, t = arguments;
    return new Promise(function(n, o) {
      var i = e.apply(r, t);
      function a(e) {}
      a(void 0);
    });
  }
}
const descriptors = [{
  key: "fetchInternal",
  value: (e = _asyncToGenerator(_regenerator().m(function e(r, t) {
    return _regenerator().w(function(e) {
      for (;;) {
        switch (e.n) {
          case 0:
            return e.n = 1, send(r, t);
          case 1:
            return e.a(2, e.v);
        }
      }
    }, e);
  })), function(r, t) {
    return e.apply(this, arguments);
  })
}];
var e;
"#;
    let expected = r#"
const descriptors = [{
  key: "fetchInternal",
  value: async function(r, t) {
    return await send(r, t);
  }
}];
var e;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn babel_729_terser_class_method_sequence_keeps_escaped_private_binding() {
    let input = r#"
function _asyncToGenerator(e) {
  return function() {
    var r = this, t = arguments;
    return new Promise(function(n, o) {
      var i = e.apply(r, t);
      function a(e) {}
      a(void 0);
    });
  }
}
const descriptors = [{
  key: "fetchInternal",
  value: (e = _asyncToGenerator(_regenerator().m(function e(r, t) {
    return _regenerator().w(function(e) {
      for (;;) {
        switch (e.n) {
          case 0:
            return e.n = 1, send(r, t);
          case 1:
            return e.a(2, e.v);
        }
      }
    }, e);
  })), function(r, t) {
    return e.apply(this, arguments);
  })
}];
var e;
use(e, descriptors);
"#;
    let output = apply(input);
    assert!(
        output.contains("e = async function"),
        "escaped private async function assignment must be preserved, got:\n{output}"
    );
    assert!(
        output.contains("use(e, descriptors)"),
        "escaped private binding use must remain valid, got:\n{output}"
    );
}

#[test]
fn async_to_generator_expression_var_init() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
const load_user = _asyncToGenerator(function* (app_id) {
  var response = yield fetch_user(app_id);
  return response;
});
"#;
    let expected = r#"
const load_user = async function(app_id) {
  var response = await fetch_user(app_id);
  return response;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_removes_top_level_step_dependency() {
    let input = r#"
function asyncGeneratorStep(n, t, e, r, o, a, c) {
  try {
    var i = n[a](c), u = i.value;
  } catch (n) {
    return void e(n);
  }
  i.done ? t(u) : Promise.resolve(u).then(r, o);
}
function _asyncToGenerator(n) {
  return function() {
    var t = this, e = arguments;
    return new Promise(function(r, o) {
      var a = n.apply(t, e);
      function _next(n) {
        asyncGeneratorStep(a, r, o, _next, _throw, "next", n);
      }
      function _throw(n) {
        asyncGeneratorStep(a, r, o, _next, _throw, "throw", n);
      }
      _next(void 0);
    });
  };
}
function loadValue(_arg) {
  return _loadValue.apply(this, arguments);
}
function _loadValue() {
  return (_loadValue = _asyncToGenerator(function* (recordId) {
    const response = yield fetchRecord(recordId);
    return yield response.json();
  })).apply(this, arguments);
}
"#;
    let expected = r#"
async function loadValue(recordId) {
  const response = await fetchRecord(recordId);
  return await response.json();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_babel_trampoline_with_params() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  _load_user = _asyncToGenerator(function* (app_id) {
    var response = yield fetch_user(app_id);
    return response;
  });
  return _load_user.apply(this, arguments);
}
"#;
    let expected = r#"
async function load_user(app_id) {
  var response = await fetch_user(app_id);
  return response;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_babel_trampoline_with_regenerator() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  _load_user = _asyncToGenerator(regeneratorRuntime.mark(function _callee(app_id) {
    return regeneratorRuntime.wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          _context.next = 2;
          return fetch_user(app_id);
        case 2:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _load_user.apply(this, arguments);
}
"#;
    let expected = r#"
async function load_user(app_id) {
  await fetch_user(app_id);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_babel_728_trampoline_with_regenerator() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  _load_user = _asyncToGenerator(_regenerator().m(function _callee(app_id) {
    var response, data;
    return _regenerator().w(function (_context) {
      while (1) switch (_context.n) {
        case 0:
          _context.n = 1;
          return fetch_user(app_id);
        case 1:
          response = _context.v;
          _context.n = 2;
          return response.json();
        case 2:
          data = _context.v;
          return _context.a(2, data);
      }
    }, _callee);
  }));
  return _load_user.apply(this, arguments);
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
fn async_to_generator_minified_babel_728_self_rewriting_trampoline() {
    let input = r#"
function t() {
  function w() {}
  function m() {}
  n({}, "_invoke", function() {});
  return (t = function() {
    return { w: w, m: m };
  })();
}
function n(t, r, e) {
  Object.defineProperty(t, r, { value: e });
}
function r(t, n, r, e, o, i, u) {
  try {
    var c = t[i](u), a = c.value;
  } catch (t) {
    return void r(t);
  }
  c.done ? n(a) : Promise.resolve(a).then(e, o);
}
function e(t) {
  return function() {
    var n = this, o = arguments;
    return new Promise(function(i, u) {
      var c = t.apply(n, o);
      function a(t) {
        r(c, i, u, a, f, "next", t);
      }
      function f(t) {
        r(c, i, u, a, f, "throw", t);
      }
      a(void 0);
    });
  };
}
function o(t) {
  return i.apply(this, arguments);
}
function i() {
  return (i = e(t().m(function n(r) {
    return t().w(function(t) {
      for (;;) switch (t.n) {
        case 0:
          t.n = 1;
          return fetch_user(r);
        case 1:
          return t.a(2);
      }
    }, n);
  }))).apply(this, arguments);
}
"#;
    let expected = r#"
async function o(r) {
  await fetch_user(r);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_babel_trampoline_with_regenerator_try_catch() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  _load_user = _asyncToGenerator(regeneratorRuntime.mark(function _callee(app_id) {
    return regeneratorRuntime.wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          _context.prev = 0;
          _context.next = 3;
          return fetch_user(app_id);
        case 3:
          return _context.abrupt("return", _context.sent);
        case 6:
          _context.prev = 6;
          _context.t0 = _context["catch"](0);
          return _context.abrupt("return", fallback_user(_context.t0));
        case 9:
        case "end":
          return _context.stop();
      }
    }, _callee, null, [[0, 6]]);
  }));
  return _load_user.apply(this, arguments);
}
"#;
    let expected = r#"
async function load_user(app_id) {
  try {
    return await fetch_user(app_id);
  } catch (error) {
    return fallback_user(error);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_babel_728_trampoline_with_regenerator_try_catch() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  _load_user = _asyncToGenerator(_regenerator().m(function _callee(app_id) {
    var _t;
    return _regenerator().w(function (_context) {
      while (1) switch (_context.p = _context.n) {
        case 0:
          _context.p = 0;
          _context.n = 1;
          return fetch_user(app_id);
        case 1:
          return _context.a(2, _context.v);
        case 2:
          _context.p = 2;
          _t = _context.v;
          return _context.a(2, fallback_user(_t));
      }
    }, _callee, null, [[0, 2]]);
  }));
  return _load_user.apply(this, arguments);
}
"#;
    let expected = r#"
async function load_user(app_id) {
  var _t;
  try {
    return await fetch_user(app_id);
  } catch (error) {
    return fallback_user(error);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_babel_trampoline_with_regenerator_try_catch_then_return() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  _load_user = _asyncToGenerator(regeneratorRuntime.mark(function _callee(app_id) {
    return regeneratorRuntime.wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          _context.prev = 0;
          _context.next = 3;
          return fetch_user(app_id);
        case 3:
          _context.next = 8;
          break;
        case 5:
          _context.prev = 5;
          _context.t0 = _context["catch"](0);
          fallback_user(_context.t0);
        case 8:
          return _context.abrupt("return", done());
        case 9:
        case "end":
          return _context.stop();
      }
    }, _callee, null, [[0, 5]]);
  }));
  return _load_user.apply(this, arguments);
}
"#;
    let expected = r#"
async function load_user(app_id) {
  try {
    await fetch_user(app_id);
  } catch (error) {
    fallback_user(error);
  }
  return done();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_babel_loop_try_catch_recovers_index_loop() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function collect_enabled(_x) {
  return _collect_enabled.apply(this, arguments);
}
function _collect_enabled() {
  _collect_enabled = _asyncToGenerator(regeneratorRuntime.mark(function _callee(items) {
    var output, index, item;
    return regeneratorRuntime.wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          output = [];
          index = 0;
        case 2:
          if (!(index < items.length)) {
            _context.next = 24;
            break;
          }
          item = items[index];
          if (item.enabled) {
            _context.next = 6;
            break;
          }
          return _context.abrupt("continue", 21);
        case 6:
          _context.prev = 6;
          _context.t0 = output;
          _context.next = 10;
          return fetch_item(item.id);
        case 10:
          _context.t1 = _context.sent;
          _context.t0.push.call(_context.t0, _context.t1);
          _context.next = 21;
          break;
        case 14:
          _context.prev = 14;
          _context.t2 = _context["catch"](6);
          _context.t3 = output;
          _context.next = 19;
          return recover_item(item, _context.t2);
        case 19:
          _context.t4 = _context.sent;
          _context.t3.push.call(_context.t3, _context.t4);
        case 21:
          index++;
          _context.next = 2;
          break;
        case 24:
          return _context.abrupt("return", output);
        case 25:
        case "end":
          return _context.stop();
      }
    }, _callee, null, [[6, 14]]);
  }));
  return _collect_enabled.apply(this, arguments);
}
"#;
    let expected = r#"
async function collect_enabled(items) {
  var output, index, item;
  output = [];
  index = 0;
  for (; index < items.length; index++) {
    item = items[index];
    if (!item.enabled) continue;
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
fn async_to_generator_babel_728_loop_try_catch_recovers_index_loop() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function collect_enabled(_x) {
  return _collect_enabled.apply(this, arguments);
}
function _collect_enabled() {
  _collect_enabled = _asyncToGenerator(_regenerator().m(function _callee(items) {
    var output, index, item, _t, _t2, _t3;
    return _regenerator().w(function (_context) {
      while (1) switch (_context.p = _context.n) {
        case 0:
          output = [];
          index = 0;
        case 1:
          if (!(index < items.length)) {
            _context.n = 7;
            break;
          }
          item = items[index];
          if (item.enabled) {
            _context.n = 2;
            break;
          }
          return _context.a(3, 6);
        case 2:
          _context.p = 2;
          _t = output;
          _context.n = 3;
          return fetch_item(item.id);
        case 3:
          _t.push.call(_t, _context.v);
          _context.n = 6;
          break;
        case 4:
          _context.p = 4;
          _t2 = _context.v;
          _t3 = output;
          _context.n = 5;
          return recover_item(item, _t2);
        case 5:
          _t3.push.call(_t3, _context.v);
        case 6:
          index++;
          _context.n = 1;
          break;
        case 7:
          return _context.a(2, output);
      }
    }, _callee, null, [[2, 4]]);
  }));
  return _collect_enabled.apply(this, arguments);
}
"#;
    let expected = r#"
async function collect_enabled(items) {
  var output, index, item, _t, _t2, _t3;
  output = [];
  index = 0;
  for (; index < items.length; index++) {
    item = items[index];
    if (!item.enabled) continue;
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
fn local_temp_member_call_keeps_later_temp_read() {
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(receiver, arg) {
  var _t;
  return regeneratorRuntime.wrap(function(_context) {
    while (true) {
      switch (_context.prev = _context.next) {
        case 0:
          _t = receiver;
          _t.method.call(_t, arg);
          return _context.abrupt("return", _t);
        case "end":
          return _context.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen(receiver, arg) {
  var _t;
  _t = receiver;
  _t.method.call(_t, arg);
  return _t;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_expression_assignment_with_regenerator_try_catch() {
    let input = r#"
const runtime = interop(require("./module-runtime.js"));
const asyncHelper = interop(require("./module-async.js"));
function wrap_handlers(app_info, logger) {
  return Object.keys(app_info).reduce((handlers, key) => {
    handlers[key] = asyncHelper.default(runtime.default.mark(function handler() {
      let current;
      const args = arguments;
      return runtime.default.wrap(function(_context) {
        while (1) switch (_context.prev = _context.next) {
          case 0:
            _context.prev = 0;
            current = app_info[key];
            _context.next = 4;
            return current(...args);
          case 4:
            return _context.abrupt("return", _context.sent);
          case 7:
            _context.prev = 7;
            _context.t0 = _context.catch(0);
            logger.error(key, _context.t0);
          case 10:
          case "end":
            return _context.stop();
        }
      }, handler, null, [[0, 7]]);
    }));
    return handlers;
  }, {});
}
"#;
    let expected = r#"
const runtime = interop(require("./module-runtime.js"));
const asyncHelper = interop(require("./module-async.js"));
function wrap_handlers(app_info, logger) {
  return Object.keys(app_info).reduce((handlers, key) => {
    handlers[key] = async function handler() {
      let current;
      const args = arguments;
      try {
        current = app_info[key];
        return await current(...args);
      } catch (error) {
        logger.error(key, error);
      }
    };
    return handlers;
  }, {});
}
"#;
    let output = apply_with_helper_facts(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn cross_module_async_to_generator_with_exported_public_trampoline() {
    let input = r#"
const runtime = interop(require("./module-runtime.js"));
const asyncHelper = interop(require("./module-async.js"));
export function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  _load_user = asyncHelper.default(runtime.default.mark(function _callee(app_id) {
    return runtime.default.wrap(function(_context) {
      while (true) {
        switch (_context.prev = _context.next) {
          case 0:
            _context.next = 2;
            return fetch_user(app_id);
          case 2:
            return _context.abrupt("return", _context.sent);
          case 3:
          case "end":
            return _context.stop();
        }
      }
    }, _callee);
  }));
  return _load_user.apply(this, arguments);
}
"#;
    let expected = r#"
const runtime = interop(require("./module-runtime.js"));
const asyncHelper = interop(require("./module-async.js"));
export async function load_user(app_id) {
  return await fetch_user(app_id);
}
"#;
    let output = apply_with_helper_facts(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn cross_module_async_to_generator_with_compact_private_trampoline() {
    let input = r#"
const runtime = interop(require("./module-runtime.js"));
const asyncHelper = interop(require("./module-async.js"));
export function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  return (_load_user = asyncHelper.default(runtime.default.mark(function _callee(app_id) {
    return runtime.default.wrap(function(_context) {
      while (true) {
        switch (_context.prev = _context.next) {
          case 0:
            _context.next = 2;
            return fetch_user(app_id);
          case 2:
            return _context.abrupt("return", _context.sent);
          case 3:
          case "end":
            return _context.stop();
        }
      }
    }, _callee);
  }))).apply(this, arguments);
}
"#;
    let expected = r#"
const runtime = interop(require("./module-runtime.js"));
const asyncHelper = interop(require("./module-async.js"));
export async function load_user(app_id) {
  return await fetch_user(app_id);
}
"#;
    let output = apply_with_helper_facts(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn missing_cross_module_helper_fact_keeps_async_wrapper() {
    let input = r#"
const runtime = interop(require("./module-runtime.js"));
const asyncHelper = interop(require("./module-async.js"));
export function load_user(_x) {
  return _load_user.apply(this, arguments);
}
function _load_user() {
  _load_user = asyncHelper.default(runtime.default.mark(function _callee(app_id) {
    return runtime.default.wrap(function(_context) {
      while (true) {
        switch (_context.prev = _context.next) {
          case 0:
            _context.next = 2;
            return fetch_user(app_id);
          case 2:
          case "end":
            return _context.stop();
        }
      }
    }, _callee);
  }));
  return _load_user.apply(this, arguments);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("asyncHelper.default"),
        "should require helper facts before treating member callee as async helper, got:\n{output}"
    );
}

#[test]
fn shadowed_require_does_not_enable_cross_module_async_helper_fact() {
    let input = r#"
function require(path) {
  return load(path);
}
const runtime = interop(require("./module-runtime.js"));
const asyncHelper = interop(require("./module-async.js"));
function load_user() {
  return asyncHelper.default(runtime.default.mark(function _callee() {
    return runtime.default.wrap(function(_context) {
      while (true) {
        switch (_context.prev = _context.next) {
          case 0:
            _context.next = 2;
            return fetch_user();
          case 2:
            return _context.abrupt("return", _context.sent);
          case 3:
          case "end":
            return _context.stop();
        }
      }
    }, _callee);
  }));
}
"#;

    let output = apply_with_helper_facts(input);
    assert!(
        output.contains("asyncHelper.default(runtime.default.mark"),
        "shadowed require must not enable cross-module async helper facts:\n{output}"
    );
    assert!(
        !output.contains("async function"),
        "shadowed require must not recover async through cross-module helper facts:\n{output}"
    );
}

#[test]
fn babel_728_regenerator_function() {
    let input = r#"
function read_items(items) {
  return _regenerator().w(function (_context) {
    while (1) switch (_context.n) {
      case 0:
        _context.n = 1;
        return first_item(items);
      case 1:
        _context.n = 2;
        return second_item(items);
      case 2:
        return _context.a(2);
    }
  }, read_items);
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
fn no_transform_non_regenerator() {
    // Should not transform regular functions
    let input = r#"
function normal() {
  return someCall.wrap(function(x) {
    console.log(x);
  });
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// ── P1 regression tests ─────────────────────────────────────────────────────

#[test]
fn bail_on_nested_control_flow() {
    // Conditional jumps (if/else with _ctx.next) produce invalid output when
    // linearized — the rule must bail out and leave the code untouched.
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(cond) {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          if (!cond) {
            e.next = 3;
            break;
          }
          e.next = 2;
          return a;
        case 2:
          e.next = 4;
          return b;
        case 3:
          e.next = 5;
          return c;
        case 4:
        case 5:
        case "end":
          return e.stop();
      }
    }
  }, _marked, this);
}
"#;
    let output = apply(input);
    // The function must NOT be converted to function* — it should be left as-is
    assert!(
        output.contains("regeneratorRuntime"),
        "should bail out when nested control flow is detected, got:\n{output}"
    );
}

#[test]
fn nested_callback_param_sharing_the_state_name_is_not_state_control_flow() {
    // The callback's `e` is a different binding from the state parameter `e`.
    // Its `.next` assignment inside the `if` block is ordinary code, not a
    // state jump, so the generator must still be recovered.
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(items) {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          if (items) {
            items.forEach(function(e) {
              e.next = null;
            });
          }
          e.next = 3;
          return first;
        case 3:
        case "end":
          return e.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen(items) {
  if (items) {
    items.forEach(function(e) {
      e.next = null;
    });
  }
  yield first;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn nested_callback_param_sharing_the_state_name_is_not_a_catch_call() {
    // `e.catch(noop)` belongs to the callback's own `e`, so the missing
    // try-region table does not block recovery.
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(promise) {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          e.next = 2;
          return promise.then(function(e) {
            return e.catch(noop);
          });
        case 2:
        case "end":
          return e.stop();
      }
    }
  }, _marked, this);
}
"#;
    let expected = r#"
function* myGen(promise) {
  yield promise.then(function(e) {
    return e.catch(noop);
  });
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn bail_on_async_to_gen_with_inner_params() {
    // _asyncToGenerator wrapping a function with params is not real Babel output.
    // Transforming it would drop the params and leave unbound references.
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function myFunc() {
  return _asyncToGenerator(function*(x) {
    return yield x;
  })();
}
"#;
    let output = apply(input);
    // Should NOT transform — inner generator has params.
    // The original return _asyncToGenerator(...) must be preserved.
    assert!(
        !output.contains("async function myFunc"),
        "should not transform inner generator with params, got:\n{output}"
    );
    assert!(
        output.contains("_asyncToGenerator"),
        "original return statement must be preserved, got:\n{output}"
    );
}

#[test]
fn bail_on_async_to_gen_with_outer_args() {
    // _asyncToGenerator(fn)(42) — outer call has args, not safe to drop
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function myFunc() {
  return _asyncToGenerator(function*() {
    return yield fetch("/api");
  })(42);
}
"#;
    let output = apply(input);
    // Should NOT transform — outer IIFE has arguments.
    // The original return _asyncToGenerator(...)(42) must be preserved.
    assert!(
        !output.contains("async function myFunc"),
        "should not transform when outer call has args, got:\n{output}"
    );
    assert!(
        output.contains("_asyncToGenerator"),
        "original return statement must be preserved, got:\n{output}"
    );
}

#[test]
fn babel_guard_chain_joining_mid_machine_recovers_async_function() {
    // Real Babel output: three guards, each joining at a mid-machine label.
    // Every yield resumes at the next case, so the guards fold at their joins.
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function init() {
  return _asyncToGenerator(regeneratorRuntime.mark(function _callee() {
    return regeneratorRuntime.wrap(function(_context) {
      while (true) {
        switch (_context.prev = _context.next) {
          case 0:
            if (supports()) {
              _context.next = 2;
              break;
            }
            return _context.abrupt("return", Promise.resolve(true));
          case 2:
            if (!loading) {
              _context.next = 4;
              break;
            }
            return _context.abrupt("return", pending);
          case 4:
            if (existing) {
              _context.next = 12;
              break;
            }
            _context.next = 9;
            return create();
          case 9:
            existing = _context.sent;
            pending = null;
            loading = false;
          case 12:
            return _context.abrupt("return", existing);
          case 13:
          case "end":
            return _context.stop();
        }
      }
    }, _callee);
  }))();
}
"#;
    let expected = r#"
async function init() {
  if (!supports()) {
    return Promise.resolve(true);
  }
  if (loading) {
    return pending;
  }
  if (!existing) {
    existing = await create();
    pending = null;
    loading = false;
  }
  return existing;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn bail_on_async_to_gen_when_regenerator_decode_rolls_back() {
    // A machine can pass the shallow wrap-shape precheck while the
    // state-machine decoder rolls back after an unsupported jump remains:
    // here the yield in case 4 resumes at label 12 rather than the next case,
    // so the guards keep end-of-machine joins and stay opcodes. The async
    // wrapper must remain intact instead of panicking.
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function init() {
  return _asyncToGenerator(regeneratorRuntime.mark(function _callee() {
    return regeneratorRuntime.wrap(function(_context) {
      while (true) {
        switch (_context.prev = _context.next) {
          case 0:
            if (supports()) {
              _context.next = 2;
              break;
            }
            return _context.abrupt("return", Promise.resolve(true));
          case 2:
            if (!loading) {
              _context.next = 4;
              break;
            }
            return _context.abrupt("return", pending);
          case 4:
            if (existing) {
              _context.next = 12;
              break;
            }
            _context.next = 12;
            return create();
          case 9:
            pending = null;
            loading = false;
          case 12:
            return _context.abrupt("return", existing);
          case 13:
          case "end":
            return _context.stop();
        }
      }
    }, _callee);
  }))();
}
"#;
    let output = apply(input);
    assert!(
        output.contains("regeneratorRuntime.wrap"),
        "unsupported state machine should be preserved, got:\n{output}"
    );
    assert!(
        !output.contains("async function init"),
        "unsupported state machine must not be partially recovered, got:\n{output}"
    );
}

#[test]
fn no_remove_unrelated_mark_calls() {
    // var marker = tracker.mark(doSideEffect()) should NOT be removed
    let input = r#"
var marker = tracker.mark(doSideEffect());
function normal() {
  console.log(marker);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("tracker.mark"),
        "unrelated .mark() calls must not be removed, got:\n{output}"
    );
}

#[test]
fn generator_simple_for_loop_with_forward_jump() {
    let input = r#"
var _marked = regeneratorRuntime.mark(iter);
function iter(items) {
  var i;
  return regeneratorRuntime.wrap(function iter$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        i = 0;
      case 1:
        if (!(i < items.length)) {
          _context.next = 7;
          break;
        }
        _context.next = 4;
        return items[i];
      case 4:
        i++;
        _context.next = 1;
        break;
      case 7:
      case "end":
        return _context.stop();
    }
  }, _marked);
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
fn generator_simple_conditional_skip_to_end() {
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(cond) {
  return regeneratorRuntime.wrap(function(_ctx) {
    while (true) {
      switch (_ctx.prev = _ctx.next) {
        case 0:
          if (!cond) {
            _ctx.next = 3;
            break;
          }
          _ctx.next = 2;
          return doA();
        case 2:
        case 3:
        case "end":
          return _ctx.stop();
      }
    }
  }, _marked);
}
"#;
    let expected = r#"
function* myGen(cond) {
  if (cond) {
    yield doA();
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_double_await() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
function fn_name() {
  return _fn_name.apply(this, arguments);
}
function _fn_name() {
  _fn_name = _asyncToGenerator(function* () {
    yield yield 1;
  });
  return _fn_name.apply(this, arguments);
}
"#;
    let expected = r#"
async function fn_name() {
  await await 1;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_to_generator_standalone_iife() {
    let input = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
_asyncToGenerator(function*() { yield fetch("/api"); })();
"#;
    let expected = r#"
(async function() {
  await fetch("/api");
})();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn esbuild_async_standalone_iife() {
    let input = r#"
var __async = (__this, __arguments, generator) => {
  return new Promise((resolve, reject) => {
    var step = (x) => x.done ? resolve(x.value) : Promise.resolve(x.value).then(fulfilled, rejected);
    step((generator = generator.apply(__this, __arguments)).next());
  });
};
__async(null, null, function* () {
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

// ── Canonical-operand gating (dropped operands must be effect-free) ─────────

#[test]
fn preserves_wrap_on_effectful_call_receiver() {
    // Recovery drops the receiver's evaluation. A call receiver with
    // arguments is not the canonical lazy-runtime shape, so it is preserved.
    let input = r#"
function myGen() {
  return getRuntime(config).wrap(function(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        _context.next = 2;
        return 1;
      case 2:
      case "end":
        return _context.stop();
    }
  }, _marked, this);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_wrap_with_extra_arguments() {
    // The generated call has at most four arguments; a fifth is not producer
    // output and would be silently discarded by recovery.
    let input = r#"
function myGen() {
  return regeneratorRuntime.wrap(function(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        _context.next = 2;
        return 1;
      case 2:
      case "end":
        return _context.stop();
    }
  }, _marked, this, [], probe());
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_wrap_with_effectful_dropped_operand() {
    // The marker/thisArg operands are dropped without evaluation, so a call
    // in either slot must fail the match.
    let input = r#"
function myGen() {
  return regeneratorRuntime.wrap(function(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        _context.next = 2;
        return 1;
      case 2:
      case "end":
        return _context.stop();
    }
  }, probe(), this);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn recovers_wrap_with_indexed_marker_operand() {
    // Babel 6 emits `_marked[0]` markers; a literal-indexed ident chain is a
    // canonical dropped operand.
    let input = r#"
var _marked = [myGen].map(regeneratorRuntime.mark);
function myGen() {
  return regeneratorRuntime.wrap(function(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        _context.next = 2;
        return 1;
      case 2:
      case "end":
        return _context.stop();
    }
  }, _marked[0], this);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("function*"),
        "indexed marker should still recover: {output}"
    );
}

#[test]
fn preserves_wrap_with_non_canonical_try_region_entry() {
    // A try-region table entry that is not a numeric region array would be
    // dropped without evaluation; the whole recovery must fail closed.
    let input = r#"
function gen() {
  return regeneratorRuntime.wrap(function callee$(_ctx) {
    while (1) switch (_ctx.prev = _ctx.next) {
      case 0:
      case "end":
        return _ctx.stop();
    }
  }, _marked, null, [probe()]);
}
"#;
    let output = render(input);
    assert!(
        output.contains("probe()"),
        "non-canonical try-region entry must preserve the call:\n{output}"
    );
}

#[test]
fn preserves_wrap_with_spread_try_region_entry() {
    let input = r#"
function gen() {
  return regeneratorRuntime.wrap(function callee$(_ctx) {
    while (1) switch (_ctx.prev = _ctx.next) {
      case 0:
      case "end":
        return _ctx.stop();
    }
  }, _marked, null, [...regions]);
}
"#;
    let output = render(input);
    assert!(
        output.contains("regions"),
        "spread try-region table must preserve the call:\n{output}"
    );
}

#[test]
fn preserves_wrap_with_non_integer_try_region_slot() {
    let input = r#"
function gen() {
  return regeneratorRuntime.wrap(function callee$(_ctx) {
    while (1) switch (_ctx.prev = _ctx.next) {
      case 0:
      case "end":
        return _ctx.stop();
    }
  }, _marked, null, [[-1, 2.5]]);
}
"#;
    let output = render(input);
    assert!(
        output.contains("regeneratorRuntime.wrap") || output.contains(".wrap"),
        "non-integer region slots must preserve the wrapper:\n{output}"
    );
}

#[test]
fn preserves_wrap_with_missing_leading_try_region_slot() {
    let input = r#"
function gen() {
  return regeneratorRuntime.wrap(function callee$(_ctx) {
    while (1) switch (_ctx.prev = _ctx.next) {
      case 0:
      case "end":
        return _ctx.stop();
    }
  }, _marked, null, [[, 2]]);
}
"#;
    let output = render(input);
    assert!(
        output.contains(".wrap"),
        "hole in the mandatory tryLoc slot must preserve the wrapper:\n{output}"
    );
}

#[test]
fn preserves_wrap_with_out_of_range_try_region_slot() {
    let input = r#"
function gen() {
  return regeneratorRuntime.wrap(function callee$(_ctx) {
    while (1) switch (_ctx.prev = _ctx.next) {
      case 0:
      case "end":
        return _ctx.stop();
    }
  }, _marked, null, [[1e30, 2]]);
}
"#;
    let output = render(input);
    assert!(
        output.contains(".wrap"),
        "out-of-range region slot must preserve the wrapper:\n{output}"
    );
}

#[test]
fn swc_then_typescript_namespace_generator_restores_async() {
    let input = include_str!("fixtures/mixed-async/generated.js");
    let output = apply(input);
    assert!(output.contains("async function load"), "{output}");
    assert!(output.contains("await Promise.resolve(value)"), "{output}");
    assert!(!output.contains("_async_to_generator"), "{output}");
    assert!(!output.contains("tslib_1.__generator"), "{output}");
}

#[test]
fn mixed_generator_namespace_requires_unchanged_binding() {
    let input = include_str!("fixtures/mixed-async/generated.js");
    for changed in [
        input.replace("function load(value)", "function load(value, tslib_1)"),
        format!("{input}\ntslib_1 = custom;"),
    ] {
        let output = apply(&changed);
        assert!(output.contains("tslib_1.__generator"), "{output}");
        assert!(!output.contains("async function load"), "{output}");
    }
}

#[test]
fn swc_namespace_async_helpers_recover_without_calling_the_namespace() {
    for declaration in [
        "var helper = require(\"@swc/helpers/_/_async_to_generator\");",
        "const helper = require(\"@swc/helpers/_/_async_to_generator\");",
        "import * as helper from \"@swc/helpers/_/_async_to_generator\";",
    ] {
        let input = format!("{declaration} function load(value) {{ return helper._(function*() {{ return yield value; }})(); }}");
        assert_eq_normalized(
            &apply(&input),
            "async function load(value) { return await value; }",
        );
        let invalid = format!("{declaration} function load(value) {{ return helper(function*() {{ return yield value; }})(); }}");
        let output = apply(&invalid);
        assert!(!output.contains("async function load"), "{output}");
        assert!(output.contains("helper(function*"), "{output}");
    }
}

#[test]
fn swc_namespace_async_recovery_preserves_unproven_or_mutated_callees() {
    for prefix in [
        "var helper = require(\"other-package\");",
        "var helper = require(\"@swc/helpers/_/_extends\");",
        "var helper = require(\"@swc/helpers/_/_async_to_generator\"); helper = custom;",
        "var helper = require(\"@swc/helpers/_/_async_to_generator\"); var helper = custom;",
        "var helper = require(\"@swc/helpers/_/_async_to_generator\"); function replace() { helper = custom; }",
        "var helper = require(\"@swc/helpers/_/_async_to_generator\"); helper._ = custom;",
        "var helper = require(\"@swc/helpers/_/_async_to_generator\"); delete helper._;",
        "var helper = require(\"@swc/helpers/_/_async_to_generator\"); Object.defineProperty(helper, \"_\", { value: custom });",
        "var helper = require(\"@swc/helpers/_/_async_to_generator\"); with (scope) { observe(); }",
        "var helper = require(\"@swc/helpers/_/_async_to_generator\"); eval(code);",
        "function require(path) { return custom; } var helper = require(\"@swc/helpers/_/_async_to_generator\");",
    ] {
        let input = format!("{prefix} function load(value) {{ return helper._(function*() {{ return yield value; }})(); }}");
        let output = apply(&input);
        assert!(!output.contains("async function load"), "{output}");
        assert!(output.contains("helper._(function*"), "{output}");
    }
}

#[test]
fn swc_namespace_async_recovery_respects_shadowing_and_remaining_calls() {
    let prefix = "var helper = require(\"@swc/helpers/_/_async_to_generator\");";
    let input = format!("{prefix} function load(helper, value) {{ return helper._(function*() {{ return yield value; }})(); }}");
    assert!(!apply(&input).contains("async function load"));
    let input = format!("{prefix} function other(helper) {{ helper._ = custom; }} function load(value) {{ return helper._(function*() {{ return yield value; }})(); }}");
    assert!(apply(&input).contains("async function load"));
    let input = format!("{prefix} function load(value) {{ return helper._(function*() {{ return yield value; }})(); }} consume(helper._(unknown));");
    let output = apply(&input);
    assert!(output.contains("async function load"), "{output}");
    assert!(output.contains("helper._(unknown)"), "{output}");
    assert!(
        output.contains("require(\"@swc/helpers/_/_async_to_generator\")"),
        "{output}"
    );
}

#[test]
fn swc_external_then_typescript_namespace_generator_restores_async() {
    for input in [
        include_str!("fixtures/mixed-async/external-generated.js"),
        include_str!("fixtures/mixed-async/external-es2015.js"),
    ] {
        let output = render(input);
        assert!(output.contains("async function load"), "{output}");
        assert!(output.contains("await Promise.resolve(value)"), "{output}");
        assert!(!output.contains("_async_to_generator"), "{output}");
        assert!(!output.contains("__generator"), "{output}");
    }
}

#[test]
fn swc_async_namespace_pipeline_retains_mutations_and_unsupported_calls() {
    let prefix = r#"var helper = require("@swc/helpers/_/_async_to_generator");"#;
    for effect in [
        "helper = custom;",
        "helper._ = custom;",
        "delete helper._;",
        "eval(code);",
    ] {
        let input = format!("{prefix} {effect} exports.load = function(value) {{ return helper._(function*() {{ return yield value; }})(); }};");
        let output = render(&input);
        assert!(!output.contains("async function"), "{output}");
        assert!(
            output.contains("require(\"@swc/helpers/_/_async_to_generator\")"),
            "{output}"
        );
        assert!(output.contains("helper._(function*"), "{output}");
    }
    let input = format!("{prefix} function load(value) {{ return helper._(function*() {{ return yield value; }})(); }} consume(helper._(unknown));");
    let output = render(&input);
    assert!(output.contains("async function load"), "{output}");
    assert!(output.contains("import * as helper"), "{output}");
    assert!(output.contains("helper._(unknown)"), "{output}");
    let input = format!("{prefix} exports.load = function(value) {{ return helper(function*() {{ return yield value; }})(); }};");
    let output = render(&input);
    assert!(!output.contains("async function"), "{output}");
    assert!(output.contains("helper(function*"), "{output}");
}

#[test]
fn catch_binding_avoids_names_the_machine_already_spells() {
    // `error` is a parameter the catch body reads; the synthesized catch
    // parameter must not reuse its spelling.
    let input = r#"
var _marked = regeneratorRuntime.mark(g);
function g(error) {
  return regeneratorRuntime.wrap(function(_ctx) {
    while (true) {
      switch (_ctx.prev = _ctx.next) {
        case 0:
          _ctx.prev = 0;
          _ctx.next = 3;
          return doThing();
        case 3:
          _ctx.next = 8;
          break;
        case 5:
          _ctx.prev = 5;
          _ctx.t0 = _ctx.catch(0);
          handle(_ctx.t0, error);
        case 8:
        case "end":
          return _ctx.stop();
      }
    }
  }, _marked, null, [[0, 5]]);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("catch (error_1)"),
        "catch binding should avoid the spelled `error`, got:\n{output}"
    );
    assert!(
        output.contains("handle(error_1, error)"),
        "caught value should use the fresh name, got:\n{output}"
    );
}

// ── try regions entered through a conditional jump ──────────────────────────

#[test]
fn guarded_try_catch_stays_inside_its_branch() {
    // regenerator output for `if (loader.lazy) { try { yield ... } catch
    // (error) { ... } } else { ... }`. The try entry `_context.prev = 1` sits
    // in the middle of case 0 because nothing jumps to label 1; the region
    // must still be rebuilt inside the guarded branch instead of being
    // dropped, which would run the catch body after every yield.
    let input = r#"
var _marked = regeneratorRuntime.mark(load_resource);
function load_resource(loader, path, options) {
  return regeneratorRuntime.wrap(function load_resource$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        if (!loader.lazy) {
          _context.next = 11;
          break;
        }
        _context.prev = 1;
        _context.next = 4;
        return loader.load(path, options);
      case 4:
        _context.next = 9;
        break;
      case 6:
        _context.prev = 6;
        _context.t0 = _context["catch"](1);
        report_error(_context.t0);
      case 9:
        _context.next = 12;
        break;
      case 11:
        loader.load(path, options).catch(report_error);
      case 12:
      case "end":
        return _context.stop();
    }
  }, _marked, null, [[1, 6]]);
}
"#;
    let expected = r#"
function* load_resource(loader, path, options) {
  if (loader.lazy) {
    try {
      yield loader.load(path, options);
    } catch (error) {
      report_error(error);
    }
  } else {
    loader.load(path, options).catch(report_error);
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn guarded_try_catch_without_else_is_recovered() {
    let input = r#"
var _marked = regeneratorRuntime.mark(load_resource);
function load_resource(loader, path, options) {
  return regeneratorRuntime.wrap(function load_resource$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        if (!loader.lazy) {
          _context.next = 9;
          break;
        }
        _context.prev = 1;
        _context.next = 4;
        return loader.load(path, options);
      case 4:
        _context.next = 9;
        break;
      case 6:
        _context.prev = 6;
        _context.t0 = _context["catch"](1);
        report_error(_context.t0);
      case 9:
      case "end":
        return _context.stop();
    }
  }, _marked, null, [[1, 6]]);
}
"#;
    let expected = r#"
function* load_resource(loader, path, options) {
  if (loader.lazy) {
    try {
      yield loader.load(path, options);
    } catch (error) {
      report_error(error);
    }
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn guarded_try_catch_stays_inside_its_branch_in_new_runtime() {
    // Babel 7.28+ `_regenerator().w` shape of the same source: `_context.p = 1`
    // marks the try entry mid-case, the caught value arrives in `_context.v`.
    let input = r#"
var _marked = _regenerator().m(load_resource);
function load_resource(loader, path, options) {
  var _t;
  return _regenerator().w(function (_context) {
    while (1) switch (_context.p = _context.n) {
      case 0:
        if (!loader.lazy) {
          _context.n = 5;
          break;
        }
        _context.p = 1;
        _context.n = 2;
        return loader.load(path, options);
      case 2:
        _context.n = 4;
        break;
      case 3:
        _context.p = 3;
        _t = _context.v;
        report_error(_t);
      case 4:
        _context.n = 6;
        break;
      case 5:
        loader.load(path, options).catch(report_error);
      case 6:
        return _context.a(2);
    }
  }, _marked, null, [[1, 3]]);
}
"#;
    let expected = r#"
function* load_resource(loader, path, options) {
  var _t;
  if (loader.lazy) {
    try {
      yield loader.load(path, options);
    } catch (error) {
      report_error(error);
    }
  } else {
    loader.load(path, options).catch(report_error);
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn guarded_try_catch_without_else_is_recovered_in_new_runtime() {
    let input = r#"
var _marked = _regenerator().m(load_resource);
function load_resource(loader, path, options) {
  var _t;
  return _regenerator().w(function (_context) {
    while (1) switch (_context.p = _context.n) {
      case 0:
        if (!loader.lazy) {
          _context.n = 4;
          break;
        }
        _context.p = 1;
        _context.n = 2;
        return loader.load(path, options);
      case 2:
        _context.n = 4;
        break;
      case 3:
        _context.p = 3;
        _t = _context.v;
        report_error(_t);
      case 4:
        return _context.a(2);
    }
  }, _marked, null, [[1, 3]]);
}
"#;
    let expected = r#"
function* load_resource(loader, path, options) {
  var _t;
  if (loader.lazy) {
    try {
      yield loader.load(path, options);
    } catch (error) {
      report_error(error);
    }
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn try_entry_after_statement_keeps_yield_inside_try() {
    // `started = start_timer(); try { yield ... } catch ...`: regenerator
    // numbers the try entry (`_context.prev = 1`) after the first statement
    // without starting a new case, so the yield belongs to label 1, inside
    // the region, not to case 0.
    let input = r#"
var _marked = regeneratorRuntime.mark(load_resource);
function load_resource(loader, path) {
  var started;
  return regeneratorRuntime.wrap(function load_resource$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        started = start_timer();
        _context.prev = 1;
        _context.next = 4;
        return loader.load(path);
      case 4:
        _context.next = 9;
        break;
      case 6:
        _context.prev = 6;
        _context.t0 = _context["catch"](1);
        report_error(_context.t0, started);
      case 9:
      case "end":
        return _context.stop();
    }
  }, _marked, null, [[1, 6]]);
}
"#;
    let expected = r#"
function* load_resource(loader, path) {
  var started;
  started = start_timer();
  try {
    yield loader.load(path);
  } catch (error) {
    report_error(error, started);
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn try_entry_after_statement_keeps_yield_inside_try_in_new_runtime() {
    let input = r#"
var _marked = _regenerator().m(load_resource);
function load_resource(loader, path) {
  var started, _t;
  return _regenerator().w(function (_context) {
    while (1) switch (_context.p = _context.n) {
      case 0:
        started = start_timer();
        _context.p = 1;
        _context.n = 2;
        return loader.load(path);
      case 2:
        _context.n = 4;
        break;
      case 3:
        _context.p = 3;
        _t = _context.v;
        report_error(_t, started);
      case 4:
        return _context.a(2);
    }
  }, _marked, null, [[1, 3]]);
}
"#;
    let expected = r#"
function* load_resource(loader, path) {
  var started, _t;
  started = start_timer();
  try {
    yield loader.load(path);
  } catch (error) {
    report_error(error, started);
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn context_temp_slots_become_locals() {
    // Babel parks a value that must survive a yield in `_context.tN`. Once the
    // state callback is gone those slots have no object to live on; each one
    // becomes a local of the recovered function.
    let input = r#"
function fetch_json() {
  return _fetch_json.apply(this, arguments);
}
function _fetch_json() {
  _fetch_json = _asyncToGenerator(regeneratorRuntime.mark(function _callee(url) {
    var response, payload;
    return regeneratorRuntime.wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          _context.next = 2;
          return fetch(url);
        case 2:
          response = _context.sent;
          if (!is_json(response)) {
            _context.next = 9;
            break;
          }
          _context.next = 6;
          return response.json();
        case 6:
          _context.t0 = _context.sent;
          _context.next = 12;
          break;
        case 9:
          _context.next = 11;
          return response.text();
        case 11:
          _context.t0 = _context.sent;
        case 12:
          payload = _context.t0;
          return _context.abrupt("return", { data: payload });
        case 14:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _fetch_json.apply(this, arguments);
}
"#;
    let output = render(input);
    assert!(!output.contains("_context"), "{output}");
    assert!(output.contains("let t0;"), "{output}");
    assert!(output.contains("t0 = yield response.json();"), "{output}");
    assert!(output.contains("t0 = yield response.text();"), "{output}");
    assert!(output.contains("payload = t0;"), "{output}");
}

#[test]
fn recovered_async_function_keeps_an_exported_helper() {
    let input = r#"
var __async = (__this, __arguments, generator) => new Promise((resolve) => {
  step((generator = generator.apply(__this, __arguments)).next());
});
function load_user(app_id) {
  return __async(this, arguments, function* () { return yield fetch_user(app_id); });
}
export { __async };
"#;
    let output = apply(input);
    assert!(output.contains("async function load_user"), "{output}");
    assert!(output.contains("var __async ="), "{output}");
    assert!(output.contains("export { __async }"), "{output}");
}

#[test]
fn generator_drops_discarded_catch_value_read() {
    // Babel 7.28 stores the caught value in `_t`. When the catch body never
    // uses it, Terser drops the unused temp and keeps a bare `_context.v`
    // read. That read is how the machine hands over the error, not a
    // statement of the original catch body.
    let input = r#"
var _marked = _regenerator().m(toggle);
function toggle(p) {
  var t;
  return _regenerator().w(function (_context) {
    while (1) switch (_context.p = _context.n) {
      case 0:
        t = p.paused;
        _context.p = 1;
        _context.n = 2;
        return p.play();
      case 2:
        _context.n = 4;
        break;
      case 3:
        _context.p = 3;
        _context.v;
      case 4:
        if (t) p.pause();
      case 5:
        return _context.a(2);
    }
  }, _marked, null, [[1, 3]]);
}
"#;
    let expected = r#"
function* toggle(p) {
  var t;
  t = p.paused;
  try {
    yield p.play();
  } catch (error) {}
  if (t) p.pause();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

const BABEL_ASYNC_TO_GENERATOR: &str = r#"
function _asyncToGenerator(fn) {
  return function() {
    var gen = fn.apply(this, arguments);
    return new Promise(function(resolve, reject) {
      function step(key, arg) {
        var info = gen[key](arg);
        if (info.done) { resolve(info.value); } else { Promise.resolve(info.value).then(_next, _throw); }
      }
      function _next(value) { step("next", value); }
      function _throw(err) { step("throw", err); }
      _next(undefined);
    });
  };
}
"#;

#[test]
fn async_to_generator_renames_generator_local_colliding_with_outer_param() {
    // A minifier reuses the outer parameter name for a generator-local
    // `var`. Flattening must not redeclare the parameter.
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
function load(e, n) {
  var t = this;
  return _asyncToGenerator(function*() {
    var n;
    n = yield t.fetch(e);
    return n;
  })();
}
"#
    );
    let expected = r#"
async function load(e, n) {
  var t = this;
  var n1;
  n1 = await t.fetch(e);
  return n1;
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn swc_async_to_generator_renames_ts_generator_local_colliding_with_outer_param() {
    let input = r#"
function _async_to_generator(fn) {
  return function() {
    var self = this, args = arguments;
    return new Promise(function(resolve, reject) {
      var gen = fn.apply(self, args);
      function _next(value) {
        resolve(gen.next(value).value);
      }
      _next(undefined);
    });
  };
}
function _ts_generator(thisArg, body) {
  var t, _ = {
    label: 0,
    sent: function() { return t[1]; },
    trys: [],
    ops: []
  };
}
function thumbnail(e, n) {
  var t = this;
  return _async_to_generator(function() {
    var n;
    return _ts_generator(this, function(r) {
      switch (r.label) {
        case 0:
          return [4, t.segment(e)];
        case 1:
          if (!(n = r.sent())) return [2, null];
          return [2, n.url];
      }
    });
  })();
}
"#;
    let output = apply(input);
    assert!(
        output.contains("async function thumbnail(e, n)")
            && output.contains("var n1;")
            && output.contains("n1 = await t.segment(e)")
            && output.contains("return n1.url"),
        "generator local must be renamed away from the outer parameter, got:\n{output}"
    );
}

#[test]
fn async_to_generator_with_colliding_local_and_eval_fails_closed() {
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
function load(e, n) {
  return _asyncToGenerator(function*() {
    var n;
    eval("n");
    n = yield fetch(e);
    return n;
  })();
}
"#
    );
    let output = apply(&input);
    assert!(
        output.contains("function load(e, n)")
            && !output.contains("async function load")
            && output.contains("var n;"),
        "direct eval must keep the generator boundary, got:\n{output}"
    );
}

// ── forward guards that join mid-machine ────────────────────────────────────
//
// Shapes from `producer @babel/preset-env@7.23.9 targets ie 11, modules false`
// (inline `_regeneratorRuntime()` helper, `_context.next`) and
// `producer @babel/preset-env@7.29.0` (`_regenerator().w`, `_context.n`), with
// the async-function trampoline kept and the runtime helper bodies left out.

#[test]
fn babel_723_early_return_guard_recovers_async_function() {
    // `if (!a) return "guard-empty"; await tick("early"); return a;`
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
export function early(_x) {
  return _early.apply(this, arguments);
}
function _early() {
  _early = _asyncToGenerator(_regeneratorRuntime().mark(function _callee(a) {
    return _regeneratorRuntime().wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          if (a) {
            _context.next = 2;
            break;
          }
          return _context.abrupt("return", "guard-empty");
        case 2:
          _context.next = 4;
          return tick("early");
        case 4:
          return _context.abrupt("return", a);
        case 5:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _early.apply(this, arguments);
}
"#
    );
    let expected = r#"
export async function early(a) {
  if (!a) {
    return "guard-empty";
  }
  await tick("early");
  return a;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn babel_723_if_await_without_else_recovers_async_function() {
    // `if (b) { await tick("guarded"); } return a + 1;`
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
export function guarded(_x, _x2) {
  return _guarded.apply(this, arguments);
}
function _guarded() {
  _guarded = _asyncToGenerator(_regeneratorRuntime().mark(function _callee(a, b) {
    return _regeneratorRuntime().wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          if (!b) {
            _context.next = 3;
            break;
          }
          _context.next = 3;
          return tick("guarded");
        case 3:
          return _context.abrupt("return", a + 1);
        case 4:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _guarded.apply(this, arguments);
}
"#
    );
    let expected = r#"
export async function guarded(a, b) {
  if (b) {
    await tick("guarded");
  }
  return a + 1;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn babel_729_early_return_guard_recovers_async_function() {
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
export function early(_x) {
  return _early.apply(this, arguments);
}
function _early() {
  _early = _asyncToGenerator(_regenerator().m(function _callee(a) {
    return _regenerator().w(function (_context) {
      while (1) switch (_context.n) {
        case 0:
          if (a) {
            _context.n = 1;
            break;
          }
          return _context.a(2, "guard-empty");
        case 1:
          _context.n = 2;
          return tick("early");
        case 2:
          return _context.a(2, a);
      }
    }, _callee);
  }));
  return _early.apply(this, arguments);
}
"#
    );
    let expected = r#"
export async function early(a) {
  if (!a) {
    return "guard-empty";
  }
  await tick("early");
  return a;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn babel_729_if_await_without_else_recovers_async_function() {
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
export function guarded(_x, _x2) {
  return _guarded.apply(this, arguments);
}
function _guarded() {
  _guarded = _asyncToGenerator(_regenerator().m(function _callee(a, b) {
    return _regenerator().w(function (_context) {
      while (1) switch (_context.n) {
        case 0:
          if (!b) {
            _context.n = 1;
            break;
          }
          _context.n = 1;
          return tick("guarded");
        case 1:
          return _context.a(2, a + 1);
      }
    }, _callee);
  }));
  return _guarded.apply(this, arguments);
}
"#
    );
    let expected = r#"
export async function guarded(a, b) {
  if (b) {
    await tick("guarded");
  }
  return a + 1;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn guard_inside_index_loop_is_not_read_as_continue() {
    // `for (var i = 0; i < n; i++) { if (a) { await x(i); } await y(i); }`.
    // The guard's join (label 5) is the loop's second await, not its update:
    // reading the guard as `continue` would skip `await y(i)`.
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
function run(_x, _x2) {
  return _run.apply(this, arguments);
}
function _run() {
  _run = _asyncToGenerator(_regeneratorRuntime().mark(function _callee(n, a) {
    var i;
    return _regeneratorRuntime().wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          i = 0;
        case 1:
          if (!(i < n)) {
            _context.next = 10;
            break;
          }
          if (!a) {
            _context.next = 5;
            break;
          }
          _context.next = 5;
          return x(i);
        case 5:
          _context.next = 7;
          return y(i);
        case 7:
          i++;
          _context.next = 1;
          break;
        case 10:
          return _context.abrupt("return", 1);
        case 11:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _run.apply(this, arguments);
}
"#
    );
    let expected = r#"
async function run(n, a) {
  let i = 0;
  for (; i < n; i++) {
    if (a) {
      await x(i);
    }
    await y(i);
  }
  return 1;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn guard_around_index_loop_keeps_the_loop_inside_the_branch() {
    // `if (a) { for (var i = 0; i < n; i++) { await y(i); } } return 4;`.
    // Without the mid-machine join the guard was read as the loop test.
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
function run(_x, _x2) {
  return _run.apply(this, arguments);
}
function _run() {
  _run = _asyncToGenerator(_regeneratorRuntime().mark(function _callee(a, n) {
    var i;
    return _regeneratorRuntime().wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          if (!a) {
            _context.next = 8;
            break;
          }
          i = 0;
        case 2:
          if (!(i < n)) {
            _context.next = 8;
            break;
          }
          _context.next = 5;
          return y(i);
        case 5:
          i++;
          _context.next = 2;
          break;
        case 8:
          return _context.abrupt("return", 4);
        case 9:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _run.apply(this, arguments);
}
"#
    );
    let expected = r#"
async function run(a, n) {
  let i;
  if (a) {
    i = 0;
    for (; i < n; i++) {
      await y(i);
    }
  }
  return 4;
}
"#;
    assert_eq_normalized(&render(&input), expected);
}

#[test]
fn mid_machine_guard_with_non_adjacent_yield_resume_fails_closed() {
    // The guard at label 0 joins at label 3, but the yield in case 2 resumes
    // at label 5, skipping case 3. Folding the guard up to label 3 would run
    // `yield c` after `yield b`.
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(cond) {
  return regeneratorRuntime.wrap(function(e) {
    while (true) {
      switch (e.prev = e.next) {
        case 0:
          if (!cond) {
            e.next = 3;
            break;
          }
          e.next = 2;
          return a;
        case 2:
          e.next = 5;
          return b;
        case 3:
          e.next = 5;
          return c;
        case 5:
          return e.abrupt("return", d);
        case 6:
        case "end":
          return e.stop();
      }
    }
  }, _marked, this);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("regeneratorRuntime"),
        "a yield that skips the join must keep the machine, got:\n{output}"
    );
}

#[test]
fn mid_machine_guard_with_comma_yield_skipping_the_join_fails_closed() {
    // Same control flow through the comma form `return e.next = N, value`.
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(cond) {
  return regeneratorRuntime.wrap(function(e) {
    for (;;) switch (e.prev = e.next) {
      case 0:
        if (!cond) {
          e.next = 3;
          break;
        }
        return e.next = 2, a;
      case 2:
        return e.next = 5, b;
      case 3:
        return e.next = 5, c;
      case 5:
        return e.abrupt("return", d);
      case 6:
      case "end":
        return e.stop();
    }
  }, _marked, this);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("regeneratorRuntime"),
        "a comma-form yield that skips the join must keep the machine, got:\n{output}"
    );
}

#[test]
fn mid_machine_guard_into_a_catch_label_fails_closed() {
    // A guard inside the try body that jumps straight to the catch label runs
    // the catch body without an exception; a rebuilt `try` cannot say that.
    let input = r#"
var _marked = regeneratorRuntime.mark(myGen);
function myGen(cond) {
  return regeneratorRuntime.wrap(function(e) {
    while (1) switch (e.prev = e.next) {
      case 0:
        e.prev = 0;
        if (cond) {
          e.next = 5;
          break;
        }
        e.next = 3;
        return a;
      case 3:
        e.next = 8;
        break;
      case 5:
        e.prev = 5;
        e.t0 = e["catch"](0);
        handle(e.t0);
      case 8:
        return e.abrupt("return", d);
      case 9:
      case "end":
        return e.stop();
    }
  }, _marked, this, [[0, 5]]);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("regeneratorRuntime"),
        "a jump into a catch label must keep the machine, got:\n{output}"
    );
}

#[test]
fn babel_729_mid_machine_void_return_keeps_the_machine() {
    // `if (!a) { log("no-a"); return; } await tick("v1");` through 7.29: the
    // value-less `_context.a(2)` decodes as the machine's final stop and is
    // dropped, so folding the guard would fall through into the yield.
    let input = r#"
var _marked = _regenerator().m(run);
function run(a) {
  return _regenerator().w(function (_context) {
    while (1) switch (_context.n) {
      case 0:
        if (a) {
          _context.n = 1;
          break;
        }
        log("no-a");
        return _context.a(2);
      case 1:
        _context.n = 2;
        return tick("v1");
      case 2:
        return _context.a(2);
    }
  }, _marked);
}
"#;
    let output = apply(input);
    assert!(
        output.contains("_regenerator().w"),
        "a mid-machine stop must keep the machine, got:\n{output}"
    );
}

#[test]
fn back_edge_to_label_zero_keeps_end_of_machine_joins() {
    // `while (a.more()) { if (a.skip) { await a.step(); } a.n++; } return 3;`:
    // the decoder drops the back-edge to label 0 and wraps the machine in
    // `while (true)`, so the loop test may not fold up to the loop exit as a
    // one-shot guard.
    let input = format!(
        "{BABEL_ASYNC_TO_GENERATOR}{}",
        r#"
function run(_x) {
  return _run.apply(this, arguments);
}
function _run() {
  _run = _asyncToGenerator(_regeneratorRuntime().mark(function _callee(a) {
    return _regeneratorRuntime().wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          if (!a.more()) {
            _context.next = 7;
            break;
          }
          if (!a.skip) {
            _context.next = 4;
            break;
          }
          _context.next = 4;
          return a.step();
        case 4:
          a.n++;
          _context.next = 0;
          break;
        case 7:
          return _context.abrupt("return", 3);
        case 8:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _run.apply(this, arguments);
}
"#
    );
    let output = render(&input);
    assert!(
        !output.contains("if (a.more())"),
        "the loop test must not become a one-shot guard, got:\n{output}"
    );
}

// ── Await-spill temps ────────────────────────────────────────────────────────

/// Recovered form of the `await_spill_*` inputs below (one source, two Babel
/// versions).
const AWAIT_SPILL_EXPECTED: &str = r#"
export async function signIn(url, opts, callbackUrl) {
  let res;
  res = await fetch(url, {
    method: "post",
    body: new URLSearchParams(merge(opts, {}, {
      csrfToken: await getToken(),
      callbackUrl,
      json: true
    }))
  });
  return res.json();
}
export async function simple(a, b) {
  return f(a, await b);
}
export async function pair(a, y) {
  let list;
  let record;
  list = [a, await y];
  record = {
    a,
    b: await y
  };
  return [list, record];
}
export async function nested(a, b, c) {
  return g(a, h(b, await c));
}
export async function make(a, b) {
  return new Widget(a, await b);
}
export async function send(client, a, b) {
  return client.post(a, await b);
}
"#;

#[test]
fn await_spill_temps_fold_back_legacy() {
    // Regenerator spills every operand evaluated before an `await` into a
    // `_context.tN` slot. Each slot is written once and read once, right
    // after the spill, so the source expression comes back with `await`
    // inline and no temp locals.
    // producer babel@7.23 preset-env (ie 11)
    let input = r#"
function asyncGeneratorStep(gen, resolve, reject, _next, _throw, key, arg) { try { var info = gen[key](arg); var value = info.value; } catch (error) { reject(error); return; } if (info.done) { resolve(value); } else { Promise.resolve(value).then(_next, _throw); } }
function _asyncToGenerator(fn) { return function () { var self = this, args = arguments; return new Promise(function (resolve, reject) { var gen = fn.apply(self, args); function _next(value) { asyncGeneratorStep(gen, resolve, reject, _next, _throw, "next", value); } function _throw(err) { asyncGeneratorStep(gen, resolve, reject, _next, _throw, "throw", err); } _next(undefined); }); }; }
export function signIn(_x, _x2, _x3) {
  return _signIn.apply(this, arguments);
}
function _signIn() {
  _signIn = _asyncToGenerator(/*#__PURE__*/_regeneratorRuntime().mark(function _callee(url, opts, cb) {
    var res;
    return _regeneratorRuntime().wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          _context.t0 = fetch;
          _context.t1 = url;
          _context.t2 = URLSearchParams;
          _context.t3 = merge;
          _context.t4 = opts;
          _context.t5 = {};
          _context.next = 8;
          return getToken();
        case 8:
          _context.t6 = _context.sent;
          _context.t7 = cb;
          _context.t8 = {
            csrfToken: _context.t6,
            callbackUrl: _context.t7,
            json: true
          };
          _context.t9 = (0, _context.t3)(_context.t4, _context.t5, _context.t8);
          _context.t10 = new _context.t2(_context.t9);
          _context.t11 = {
            method: "post",
            body: _context.t10
          };
          _context.next = 16;
          return (0, _context.t0)(_context.t1, _context.t11);
        case 16:
          res = _context.sent;
          return _context.abrupt("return", res.json());
        case 18:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _signIn.apply(this, arguments);
}
export function simple(_x4, _x5) {
  return _simple.apply(this, arguments);
}
function _simple() {
  _simple = _asyncToGenerator(/*#__PURE__*/_regeneratorRuntime().mark(function _callee2(a, b) {
    return _regeneratorRuntime().wrap(function _callee2$(_context2) {
      while (1) switch (_context2.prev = _context2.next) {
        case 0:
          _context2.t0 = f;
          _context2.t1 = a;
          _context2.next = 4;
          return b;
        case 4:
          _context2.t2 = _context2.sent;
          return _context2.abrupt("return", (0, _context2.t0)(_context2.t1, _context2.t2));
        case 6:
        case "end":
          return _context2.stop();
      }
    }, _callee2);
  }));
  return _simple.apply(this, arguments);
}
export function pair(_x6, _x7) {
  return _pair.apply(this, arguments);
}
function _pair() {
  _pair = _asyncToGenerator(/*#__PURE__*/_regeneratorRuntime().mark(function _callee3(x, y) {
    var list, record;
    return _regeneratorRuntime().wrap(function _callee3$(_context3) {
      while (1) switch (_context3.prev = _context3.next) {
        case 0:
          _context3.t0 = x;
          _context3.next = 3;
          return y;
        case 3:
          _context3.t1 = _context3.sent;
          list = [_context3.t0, _context3.t1];
          _context3.t2 = x;
          _context3.next = 8;
          return y;
        case 8:
          _context3.t3 = _context3.sent;
          record = {
            a: _context3.t2,
            b: _context3.t3
          };
          return _context3.abrupt("return", [list, record]);
        case 11:
        case "end":
          return _context3.stop();
      }
    }, _callee3);
  }));
  return _pair.apply(this, arguments);
}
export function nested(_x8, _x9, _x0) {
  return _nested.apply(this, arguments);
}
function _nested() {
  _nested = _asyncToGenerator(/*#__PURE__*/_regeneratorRuntime().mark(function _callee4(a, b, c) {
    return _regeneratorRuntime().wrap(function _callee4$(_context4) {
      while (1) switch (_context4.prev = _context4.next) {
        case 0:
          _context4.t0 = g;
          _context4.t1 = a;
          _context4.t2 = h;
          _context4.t3 = b;
          _context4.next = 6;
          return c;
        case 6:
          _context4.t4 = _context4.sent;
          _context4.t5 = (0, _context4.t2)(_context4.t3, _context4.t4);
          return _context4.abrupt("return", (0, _context4.t0)(_context4.t1, _context4.t5));
        case 9:
        case "end":
          return _context4.stop();
      }
    }, _callee4);
  }));
  return _nested.apply(this, arguments);
}
export function make(_x1, _x10) {
  return _make.apply(this, arguments);
}
function _make() {
  _make = _asyncToGenerator(/*#__PURE__*/_regeneratorRuntime().mark(function _callee5(a, b) {
    return _regeneratorRuntime().wrap(function _callee5$(_context5) {
      while (1) switch (_context5.prev = _context5.next) {
        case 0:
          _context5.t0 = Widget;
          _context5.t1 = a;
          _context5.next = 4;
          return b;
        case 4:
          _context5.t2 = _context5.sent;
          return _context5.abrupt("return", new _context5.t0(_context5.t1, _context5.t2));
        case 6:
        case "end":
          return _context5.stop();
      }
    }, _callee5);
  }));
  return _make.apply(this, arguments);
}
export function send(_x11, _x12, _x13) {
  return _send.apply(this, arguments);
}
function _send() {
  _send = _asyncToGenerator(/*#__PURE__*/_regeneratorRuntime().mark(function _callee6(client, a, b) {
    return _regeneratorRuntime().wrap(function _callee6$(_context6) {
      while (1) switch (_context6.prev = _context6.next) {
        case 0:
          _context6.t0 = client;
          _context6.t1 = a;
          _context6.next = 4;
          return b;
        case 4:
          _context6.t2 = _context6.sent;
          return _context6.abrupt("return", _context6.t0.post.call(_context6.t0, _context6.t1, _context6.t2));
        case 6:
        case "end":
          return _context6.stop();
      }
    }, _callee6);
  }));
  return _send.apply(this, arguments);
}
"#;
    assert_eq_normalized(&render(input), AWAIT_SPILL_EXPECTED);
}

#[test]
fn await_spill_temps_fold_back_compact() {
    // Babel 7.29's compact runtime spills into function-scoped `_t`, `_tN`
    // locals instead of `_context` slots.
    // producer babel@7.29 preset-env (ie 11)
    let input = r#"
function asyncGeneratorStep(n, t, e, r, o, a, c) { try { var i = n[a](c), u = i.value; } catch (n) { return void e(n); } i.done ? t(u) : Promise.resolve(u).then(r, o); }
function _asyncToGenerator(n) { return function () { var t = this, e = arguments; return new Promise(function (r, o) { var a = n.apply(t, e); function _next(n) { asyncGeneratorStep(a, r, o, _next, _throw, "next", n); } function _throw(n) { asyncGeneratorStep(a, r, o, _next, _throw, "throw", n); } _next(void 0); }); }; }
export function signIn(_x, _x2, _x3) {
  return _signIn.apply(this, arguments);
}
function _signIn() {
  _signIn = _asyncToGenerator(/*#__PURE__*/_regenerator().m(function _callee(url, opts, cb) {
    var res, _t, _t2, _t3, _t4, _t5, _t6, _t7, _t8, _t9, _t0;
    return _regenerator().w(function (_context) {
      while (1) switch (_context.n) {
        case 0:
          _t = fetch;
          _t2 = url;
          _t3 = URLSearchParams;
          _t4 = merge;
          _t5 = opts;
          _t6 = {};
          _context.n = 1;
          return getToken();
        case 1:
          _t7 = _context.v;
          _t8 = cb;
          _t9 = _t4(_t5, _t6, {
            csrfToken: _t7,
            callbackUrl: _t8,
            json: true
          });
          _t0 = new _t3(_t9);
          _context.n = 2;
          return _t(_t2, {
            method: "post",
            body: _t0
          });
        case 2:
          res = _context.v;
          return _context.a(2, res.json());
      }
    }, _callee);
  }));
  return _signIn.apply(this, arguments);
}
export function simple(_x4, _x5) {
  return _simple.apply(this, arguments);
}
function _simple() {
  _simple = _asyncToGenerator(/*#__PURE__*/_regenerator().m(function _callee2(a, b) {
    var _t1, _t10;
    return _regenerator().w(function (_context2) {
      while (1) switch (_context2.n) {
        case 0:
          _t1 = f;
          _t10 = a;
          _context2.n = 1;
          return b;
        case 1:
          return _context2.a(2, _t1(_t10, _context2.v));
      }
    }, _callee2);
  }));
  return _simple.apply(this, arguments);
}
export function pair(_x6, _x7) {
  return _pair.apply(this, arguments);
}
function _pair() {
  _pair = _asyncToGenerator(/*#__PURE__*/_regenerator().m(function _callee3(x, y) {
    var list, record, _t11, _t12, _t13, _t14;
    return _regenerator().w(function (_context3) {
      while (1) switch (_context3.n) {
        case 0:
          _t11 = x;
          _context3.n = 1;
          return y;
        case 1:
          _t12 = _context3.v;
          list = [_t11, _t12];
          _t13 = x;
          _context3.n = 2;
          return y;
        case 2:
          _t14 = _context3.v;
          record = {
            a: _t13,
            b: _t14
          };
          return _context3.a(2, [list, record]);
      }
    }, _callee3);
  }));
  return _pair.apply(this, arguments);
}
export function nested(_x8, _x9, _x0) {
  return _nested.apply(this, arguments);
}
function _nested() {
  _nested = _asyncToGenerator(/*#__PURE__*/_regenerator().m(function _callee4(a, b, c) {
    var _t15, _t16, _t17, _t18;
    return _regenerator().w(function (_context4) {
      while (1) switch (_context4.n) {
        case 0:
          _t15 = g;
          _t16 = a;
          _t17 = h;
          _t18 = b;
          _context4.n = 1;
          return c;
        case 1:
          return _context4.a(2, _t15(_t16, _t17(_t18, _context4.v)));
      }
    }, _callee4);
  }));
  return _nested.apply(this, arguments);
}
export function make(_x1, _x10) {
  return _make.apply(this, arguments);
}
function _make() {
  _make = _asyncToGenerator(/*#__PURE__*/_regenerator().m(function _callee5(a, b) {
    var _t19, _t20, _t21;
    return _regenerator().w(function (_context5) {
      while (1) switch (_context5.n) {
        case 0:
          _t19 = Widget;
          _t20 = a;
          _context5.n = 1;
          return b;
        case 1:
          _t21 = _context5.v;
          return _context5.a(2, new _t19(_t20, _t21));
      }
    }, _callee5);
  }));
  return _make.apply(this, arguments);
}
export function send(_x11, _x12, _x13) {
  return _send.apply(this, arguments);
}
function _send() {
  _send = _asyncToGenerator(/*#__PURE__*/_regenerator().m(function _callee6(client, a, b) {
    var _t22, _t23;
    return _regenerator().w(function (_context6) {
      while (1) switch (_context6.n) {
        case 0:
          _t22 = client;
          _t23 = a;
          _context6.n = 1;
          return b;
        case 1:
          return _context6.a(2, _t22.post.call(_t22, _t23, _context6.v));
      }
    }, _callee6);
  }));
  return _send.apply(this, arguments);
}
"#;
    assert_eq_normalized(&render(input), AWAIT_SPILL_EXPECTED);
}

/// A Babel 7.23 generator whose case 0 spills `f` and `a` (plus `extra`)
/// before `yield b`, and whose case 4 consumes them with `consumer`.
fn legacy_spill_generator(extra: &str, consumer: &str) -> String {
    format!(
        r#"
var _marked = _regeneratorRuntime().mark(spill);
function spill(a, b) {{
  return _regeneratorRuntime().wrap(function spill$(_context) {{
    while (1) switch (_context.prev = _context.next) {{
      case 0:
        _context.t0 = f;
        _context.t1 = a;
        {extra}
        _context.next = 4;
        return b;
      case 4:
        _context.t2 = _context.sent;
        return _context.abrupt("return", {consumer});
      case 6:
      case "end":
        return _context.stop();
    }}
  }}, _marked);
}}
"#
    )
}

#[test]
fn await_spill_fold_keeps_a_temp_read_twice() {
    let input = legacy_spill_generator(
        "",
        "(0, _context.t0)(_context.t1, _context.t2, _context.t1)",
    );
    let expected = r#"
function* spill(a, b) {
  var t0, t1, t2;
  t0 = f;
  t1 = a;
  t2 = yield b;
  return (0, t0)(t1, t2, t1);
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn await_spill_fold_keeps_temps_read_out_of_write_order() {
    let input = legacy_spill_generator("", "(0, _context.t0)(_context.t2, _context.t1)");
    let expected = r#"
function* spill(a, b) {
  var t0, t1, t2;
  t0 = f;
  t1 = a;
  t2 = yield b;
  return (0, t0)(t2, t1);
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn await_spill_fold_keeps_temps_across_an_unrelated_statement() {
    // `log()` runs between the spill and the yield; folding `f` and `a` past
    // it would read them after `log()` instead of before.
    let input = legacy_spill_generator("log();", "(0, _context.t0)(_context.t1, _context.t2)");
    let expected = r#"
function* spill(a, b) {
  var t0, t1, t2;
  t0 = f;
  t1 = a;
  log();
  t2 = yield b;
  return (0, t0)(t1, t2);
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn await_spill_fold_keeps_a_temp_read_in_a_nested_function() {
    let input = legacy_spill_generator(
        "",
        "(0, _context.t0)(function () { return _context.t1; }, _context.t2)",
    );
    let expected = r#"
function* spill(a, b) {
  var t0, t1, t2;
  t0 = f;
  t1 = a;
  t2 = yield b;
  return (0, t0)(function() {
    return t1;
  }, t2);
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn await_spill_fold_keeps_temps_written_outside_a_try() {
    // The spill is written before the try entry and read inside it.
    let input = r#"
var _marked = _regeneratorRuntime().mark(spill);
function spill(a, b) {
  return _regeneratorRuntime().wrap(function spill$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        _context.t0 = f;
        _context.t1 = a;
        _context.prev = 2;
        _context.next = 5;
        return b;
      case 5:
        _context.t2 = _context.sent;
        return _context.abrupt("return", (0, _context.t0)(_context.t1, _context.t2));
      case 9:
        _context.prev = 9;
        _context.t3 = _context["catch"](2);
        return _context.abrupt("return", g(_context.t3));
      case 12:
      case "end":
        return _context.stop();
    }
  }, _marked, null, [[2, 9]]);
}
"#;
    let expected = r#"
function* spill(a, b) {
  var t0, t1, t2;
  t0 = f;
  t1 = a;
  try {
    t2 = yield b;
    return (0, t0)(t1, t2);
  } catch (error) {
    return g(error);
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

/// A Babel 7.29 generator that stores `a` in the local `name` before
/// `yield b` and consumes it with `consumer`.
fn compact_spill_generator(name: &str, consumer: &str) -> String {
    format!(
        r#"
var _marked = _regenerator().m(spill);
function spill(a, b) {{
  var {name};
  return _regenerator().w(function (_context) {{
    while (1) switch (_context.n) {{
      case 0:
        {name} = a;
        _context.n = 1;
        return b;
      case 1:
        return _context.a(2, {consumer});
    }}
  }}, _marked);
}}
"#
    )
}

#[test]
fn await_spill_fold_compact_temp() {
    let input = compact_spill_generator("_t", "[_t, _context.v]");
    let expected = r#"
function* spill(a, b) {
  return [a, (yield b)];
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn await_spill_fold_keeps_a_compact_temp_read_twice() {
    let input = compact_spill_generator("_t", "[_t, _context.v, _t]");
    let expected = r#"
function* spill(a, b) {
  var _t;
  _t = a;
  return [_t, (yield b), _t];
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn await_spill_fold_keeps_a_local_not_named_like_a_babel_temp() {
    // A source `const head = a; return [head, await b]` compiles to the same
    // shape; only Babel's own temps are folded.
    let input = compact_spill_generator("head", "[head, _context.v]");
    let expected = r#"
function* spill(a, b) {
  var head;
  head = a;
  return [head, (yield b)];
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn await_spill_fold_wraps_a_member_value_in_callee_position() {
    // `_t = o.m; _t(...)` calls without a receiver; the folded callee keeps
    // that with `(0, o.m)`.
    let input = r#"
var _marked = _regenerator().m(spill);
function spill(o, b) {
  var _t;
  return _regenerator().w(function (_context) {
    while (1) switch (_context.n) {
      case 0:
        _t = o.m;
        _context.n = 1;
        return b;
      case 1:
        return _context.a(2, _t(_context.v));
    }
  }, _marked);
}
"#;
    let expected = r#"
function* spill(o, b) {
  return (0, o.m)((yield b));
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn await_spill_fold_inside_a_try_block() {
    // The spill and its consumer both sit inside the try region.
    // producer babel@7.23 preset-env (ie 11)
    let input = r#"
var _marked = _regeneratorRuntime().mark(spill);
function spill(a, b) {
  return _regeneratorRuntime().wrap(function spill$(_context) {
    while (1) switch (_context.prev = _context.next) {
      case 0:
        _context.prev = 0;
        _context.t0 = f;
        _context.t1 = a;
        _context.next = 5;
        return b;
      case 5:
        _context.t2 = _context.sent;
        return _context.abrupt("return", (0, _context.t0)(_context.t1, _context.t2));
      case 9:
        _context.prev = 9;
        _context.t3 = _context["catch"](0);
        return _context.abrupt("return", g(_context.t3));
      case 12:
      case "end":
        return _context.stop();
    }
  }, _marked, null, [[0, 9]]);
}
"#;
    let expected = r#"
function* spill(a, b) {
  try {
    return (0, f)(a, (yield b));
  } catch (error) {
    return g(error);
  }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}
