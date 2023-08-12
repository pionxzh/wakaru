import transform from '../un-async-await'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('restore yield* in generator',
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

inlineTest('restore await in async function',
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
inlineTest('restore await in async function with return',
`
function func(x) {
  return __awaiter(this, void 0, void 0, function () {
    var e_1;
    return __generator(this, function (_a) {
      switch (_a.label) {
        case 0: return [4 /*yield*/, 2];
        case 1:
            _a.sent();
            _a.label = 2;
        case 2:
          _a.trys.push([2, 4, 5, 6]);
          return [4 /*yield*/, x];
        case 3:
          _a.sent();
          return [3 /*break*/, 6];
        case 4:
          e_1 = _a.sent();
          console.error(e_1);
          return [3 /*break*/, 6];
        case 5:
          console.log("finally");
          return [7 /*endfinally*/];
        case 6: return [2 /*return*/];
      }
    });
  });
}
`,
`
function func(x) {
  return __awaiter(this, void 0, void 0, function*() {
    var e_1;
    yield 2;

    try {
      yield x;
    } catch (error) {
      e_1 = error;
      console.error(e_1);
    } finally {
      console.log("finally");
    }
  });
}
`,
)
