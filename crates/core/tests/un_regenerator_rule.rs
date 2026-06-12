mod common;

use common::{assert_eq_normalized, render_rule};
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
