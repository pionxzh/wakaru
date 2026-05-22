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
