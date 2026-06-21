mod common;

use common::render;

#[test]
fn recovers_async_awaiter_alias_and_removes_decl() {
    let input = r#"
const V = this && this.__awaiter || ((a, b, c, d) => { return new Promise(() => {}); });
export function foo() {
    return V(this, undefined, undefined, function*() {
        const x = yield fetch("url");
        return x;
    });
}
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn recovers_generator_alias_and_removes_decl() {
    let input = r#"
const Z = this && this.__generator || ((a, b) => {
    return b.call(a, { label: 0, sent: function() {}, trys: [], ops: [] });
});
function foo() {
    return Z(this, function(state) {
        switch(state.label) {
            case 0:
                return [2, 42];
        }
    });
}
"#;
    let output = render(input);
    assert!(
        !output.contains("__generator"),
        "__generator alias should be removed: {output}"
    );
}

#[test]
fn does_not_touch_non_helper_patterns() {
    let input = r#"
const V = someOtherThing || fallback;
function foo() {
    return V(1, 2, 3);
}
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn non_async_ts_helper_is_left_for_its_consumer() {
    let input = r#"
const Y = this && this.__assign || function() { return Object.assign.apply(Object, arguments); };
function foo(Y) {
    let Y = "";
    return Y;
}
"#;
    let output = render(input);
    assert!(
        output.contains("const Y = this && this.__assign"),
        "__assign alias should be left for the object-spread consumer: {output}"
    );
    assert!(
        output.contains("function foo(Y)") && output.contains("let Y = \"\""),
        "shadowed locals should remain untouched: {output}"
    );
}

#[test]
fn handles_let_declaration() {
    let input = r#"
let Y = this && this.__assign || function() { return Object.assign.apply(Object, arguments); };
const x = Y({}, { a: 1 });
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_canonical_helpers_from_multi_declarator_decl() {
    let input = r#"
var __awaiter = this && this.__awaiter || function(thisArg, _arguments, P, generator) {
  return new (P || (P = Promise))(function(resolve) {
    resolve(generator.apply(thisArg, _arguments || []).next());
  });
}, __generator = this && this.__generator || function(thisArg, body) {
  return body.call(thisArg, { label: 0, sent: function() {}, trys: [], ops: [] });
};
function load_user(app_id) {
  return __awaiter(this, void 0, void 0, function() {
    var response;
    return __generator(this, function(_a) {
      switch (_a.label) {
        case 0:
          return [4, fetch_user(app_id)];
        case 1:
          response = _a.sent();
          return [2, response];
      }
    });
  });
}
"#;
    let output = render(input);
    assert!(
        !output.contains("this && this.__awaiter") && !output.contains("this && this.__generator"),
        "canonical helpers in a shared var declaration should be removed: {output}"
    );
    assert!(
        output.contains("async function load_user"),
        "async function should still be recovered after helper cleanup: {output}"
    );
}

#[test]
fn late_nullish_pass_rewrites_async_recovered_ternary() {
    let input = r#"
var __awaiter = this && this.__awaiter || function(thisArg, _arguments, P, generator) {
  return new (P || (P = Promise))(function(resolve) {
    resolve(generator.apply(thisArg, _arguments || []).next());
  });
}, __generator = this && this.__generator || function(thisArg, body) {
  return body.call(thisArg, { label: 0, sent: function() {}, trys: [], ops: [] });
};
function load_user(config) {
  return __awaiter(this, void 0, void 0, function() {
    var source, tmp;
    return __generator(this, function(_a) {
      switch (_a.label) {
        case 0:
          if (config != null) return [3, 2];
          return [4, load_config()];
        case 1:
          tmp = _a.sent();
          return [3, 3];
        case 2:
          tmp = config;
        case 3:
          source = tmp;
          return [2, source];
      }
    });
  });
}
"#;
    let output = render(input);
    assert!(
        output.contains("config ?? await load_config()"),
        "late nullish pass should rewrite the conditional exposed by async recovery: {output}"
    );
    assert!(
        !output.contains("config != null ? config : await load_config()"),
        "recovered async ternary should not survive after late nullish pass: {output}"
    );
}

#[test]
fn async_recovered_object_rest_keeps_helper_identity_for_late_pass() {
    let input = r#"
var excluded = ["id", "token"];
function objectWithoutProperties(source, excluded) {
  if (source == null) return {};
  var key, index, target = objectWithoutPropertiesLoose(source, excluded);
  if (Object.getOwnPropertySymbols) {
    var symbols = Object.getOwnPropertySymbols(source);
    for (index = 0; index < symbols.length; index++) {
      key = symbols[index];
      if (excluded.indexOf(key) < 0 && Object.prototype.propertyIsEnumerable.call(source, key)) {
        target[key] = source[key];
      }
    }
  }
  return target;
}
function objectWithoutPropertiesLoose(source, excluded) {
  if (source == null) return {};
  var target = {};
  for (var key in source) {
    if (Object.prototype.hasOwnProperty.call(source, key)) {
      if (excluded.indexOf(key) >= 0) continue;
      target[key] = source[key];
    }
  }
  return target;
}
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
  _load_user = _asyncToGenerator(regeneratorRuntime.mark(function _callee(config) {
    var temp, source, id, token, options, session;
    return regeneratorRuntime.wrap(function _callee$(_context) {
      while (1) switch (_context.prev = _context.next) {
        case 0:
          if (config != null) {
            _context.next = 3;
            break;
          }
          _context.next = 2;
          return load_config();
        case 2:
          temp = _context.sent;
          _context.next = 4;
          break;
        case 3:
          temp = config;
        case 4:
          id = (source = temp).id;
          token = source.token;
          options = objectWithoutProperties(source, excluded);
          _context.next = 5;
          return open_session(token);
        case 5:
          session = _context.sent;
          return _context.abrupt("return", fetch_user(id, options, session));
        case 6:
        case "end":
          return _context.stop();
      }
    }, _callee);
  }));
  return _load_user.apply(this, arguments);
}
"#;
    let output = render(input);
    assert!(
        output.contains("async function load_user(config)"),
        "async trampoline should be recovered: {output}"
    );
    assert!(
        output.contains("{ id, token, ...options } = source"),
        "late object-rest pass should fold the async-exposed helper call: {output}"
    );
    assert!(
        !output.contains("options = objectWithoutProperties(source, excluded)"),
        "object-rest helper call should not survive after async recovery: {output}"
    );
}
