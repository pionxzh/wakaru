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
    // structure that jump back into a ternary assignment.
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
  var source, _t;
  _t = !(input == null) ? input : yield load_user();
  source = _t;
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
fn bail_on_async_to_gen_when_regenerator_decode_rolls_back() {
    // Real Babel output can pass the shallow wrap-shape precheck while the
    // state-machine decoder correctly rolls back after an unsupported jump
    // remains. The async wrapper must remain intact instead of panicking.
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
