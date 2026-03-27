mod common;

use common::{assert_eq_normalized, render};

// ── __generator only ────────────────────────────────────────────────────────

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
    let output = render(input);
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
  let x;
  let y;
  x = yield foo;
  y = yield bar;
  return y;
}
"#;
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
  let x;
  let y;
  x = await foo;
  y = await bar;
  return y;
}
"#;
    let output = render(input);
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
