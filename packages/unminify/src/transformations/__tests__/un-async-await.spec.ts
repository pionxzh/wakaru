import { wrapAstTransformation } from '@wakaru/ast-utils'
import { defineInlineTest } from '@wakaru/test-utils'
import transformAsyncAwait, { transform__awaiter, transform__generator } from '../un-async-await'

const transformAwaiter = wrapAstTransformation(transform__awaiter)
const inlineTestAwaiter = defineInlineTest(transformAwaiter)

const transformGenerator = wrapAstTransformation(transform__generator)
const inlineTestGenerator = defineInlineTest(transformGenerator)

const inlineTestAsyncAwait = defineInlineTest(transformAsyncAwait)

inlineTestGenerator('restore __generator to yield expression (simple)',
  `
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
`,
  `
function* func() {
  yield 1;
  yield 2;
  yield 3;
}
`,
)

inlineTestGenerator('restore __generator to yield expression (advanced)',
  `
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
`,
`
function func() {
  return __awaiter(this, void 0, void 0, function*() {
    var result, json;
    console.log('Before sleep');
    yield sleep(1000);
    result = yield fetch('');
    json = yield result.json();
    return json;
  });
}`,
)

// cSpell:words trys endfinally
inlineTestGenerator('restore __generator to yield expression with try/catch/finally',
`
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
        case 12:
          console.log(6);
          _a.label = 13;
        case 13:
          _a.trys.push([13, , 14, 16]);
          console.log(7);
          return [3 /*break*/, 16];
        case 14: return [4 /*yield*/, 1];
        case 15:
          _a.sent();
          console.log(8);
          return [7 /*endfinally*/];
        case 16: return [2 /*return*/];
      }
    });
  });
}
`,
`
function func(x) {
  return __awaiter(this, void 0, void 0, function*() {
    var e_1, e_2;
    yield 2;

    try {
      yield 1;
      console.log(1);
      yield x;
    } catch (error) {
      e_1 = error;
      console.error(e_1, 2);
    } finally {
      console.log("finally");
    }

    console.log(3);
    yield 7;

    try {
      console.log(4);
      yield x;
    } catch (error) {
      e_2 = error;
      console.error(e_2, 5);
    }

    console.log(6);

    try {
      console.log(7);
    } finally {
      yield 1;
      console.log(8);
    }
  });
}
`,
)

// conditional control flow
inlineTestAsyncAwait.todo('[es5-asyncFunctionConditionals.js]',
`
function conditional0() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0: return [4 /*yield*/, x];
        case 1:
          a = (_a.sent()) ? y : z;
          return [2 /*return*/];
      }
    });
  });
}
function conditional1() {
  return __awaiter(this, void 0, void 0, function () {
    var _a;
    return __generator(this, function (_b) {
      switch (_b.label) {
        case 0:
          if (!x) return [3 /*break*/, 2];
          return [4 /*yield*/, y];
        case 1:
          _a = _b.sent();
          return [3 /*break*/, 3];
        case 2:
          _a = z;
          _b.label = 3;
        case 3:
          a = _a;
          return [2 /*return*/];
      }
    });
  });
}
function conditional2() {
  return __awaiter(this, void 0, void 0, function () {
    var _a;
    return __generator(this, function (_b) {
      switch (_b.label) {
        case 0:
          if (!x) return [3 /*break*/, 1];
          _a = y;
          return [3 /*break*/, 3];
        case 1: return [4 /*yield*/, z];
        case 2:
          _a = _b.sent();
          _b.label = 3;
        case 3:
          a = _a;
          return [2 /*return*/];
      }
    });
  });
}`,
`
async function conditional0() {
  a = ((await x)) ? y : z;
}
async function conditional1() {
  a = x ? await y : z;
}

async function conditional2() {
  a = x ? y : await z;
}
`,
)

inlineTestAwaiter('restore __awaiter to async/await',
`
function func(x) {
  return __awaiter(this, void 0, void 0, function* () {
    yield 2;
    try {
      yield 1;
      console.log();
      yield x;
    }
    catch (e) {
      console.error();
    }
    finally {
      console.log("finally");
    }
    console.log();
    yield 7;
    try {
      console.log();
      yield x;
    }
    catch (e) {
      console.error(e);
    }
  });
}
`,
`
async function func(x) {
  await 2;
  try {
    await 1;
    console.log();
    await x;
  }
  catch (e) {
    console.error();
  }
  finally {
    console.log("finally");
  }
  console.log();
  await 7;
  try {
    console.log();
    await x;
  }
  catch (e) {
    console.error(e);
  }
}
`,
)

inlineTestAsyncAwait('empty async function',
`
function f() {
  return __awaiter(this, void 0, void 0, function () {
    return __generator(this, function (_a) {
      return [2 /*return*/];
    });
  });
}`,
`
async function f() {}`,
)

inlineTestAsyncAwait('restore to complete async/await',
`
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
`,
`
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
`,
)
