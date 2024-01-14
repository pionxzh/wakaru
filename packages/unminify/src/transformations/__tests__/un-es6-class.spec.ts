import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-es6-class'

const inlineTest = defineInlineTest(transform)

inlineTest('simple class declaration',
  `
var Foo = (function () {
    function Foo(name) {
        this.name = name;
        this.age = 18;
    }
    return Foo;
}());
`,
  `
class Foo {
    constructor(name) {
        this.name = name;
        this.age = 18;
    }
}
`,
)

inlineTest('advanced class declaration',
  `
var Foo = (function() {
    function t(name) {
        this.name = name;
        this.age = 18;
    }
    t.prototype.logger = function logger() {
        console.log("Hello", this.name);
    }
    t.staticMethod = function staticMethod() {
        console.log('static method')
    }
    return t;
})();
`,
  `
class Foo {
    constructor(name) {
        this.name = name;
        this.age = 18;
    }

    logger() {
        console.log("Hello", this.name);
    }

    static staticMethod() {
        console.log('static method')
    }
}
`,
)

inlineTest('Babel loose class declaration',
  `
var C = /*#__PURE__*/function () {
  function C() {
    this.field = 1;
  }
  var _proto = C.prototype;
  _proto.doSomething = function doSomething() {
    console.log(this.field);
  };
  return C;
}();
`,
  `
class C {
  constructor() {
    this.field = 1;
  }

  doSomething() {
    console.log(this.field);
  }
}
`)

inlineTest('extend super class',
  `
import babelInherits from "@babel/runtime/helpers/inherits";

var BabelSuperClass = /*#__PURE__*/_createClass(function BabelSuperClass() {
    _classCallCheck(this, BabelSuperClass);
});
var BabelSubClass = /*#__PURE__*/function (_BabelSuperClass) {
    _inherits(BabelSubClass, _BabelSuperClass);
    var _super = _createSuper(BabelSubClass);
    function BabelSubClass() {
        var _this;
        _classCallCheck(this, BabelSubClass);
        return _possibleConstructorReturn(_this);
    }
    return _createClass(BabelSubClass);
}(BabelSuperClass);

var BabelSuperClass2 = /*#__PURE__*/_createClass(function BabelSuperClass2() {
    _classCallCheck(this, BabelSuperClass2);
});
var BabelSubClass2 = /*#__PURE__*/function (_BabelSuperClass2) {
    babelInherits(BabelSubClass2, _BabelSuperClass2);
    var _super = _createSuper(BabelSubClass2);
    function BabelSubClass2() {
        var _this;
        _classCallCheck(this, BabelSubClass2);
        return _possibleConstructorReturn(_this);
    }
    return _createClass(BabelSubClass2);
}(BabelSuperClass2);

var SwcSuperClass = function SwcSuperClass() {
    "use strict";
    _class_call_check(this, SwcSuperClass);
};
var SwcSubClass = /*#__PURE__*/ function(SwcSuperClass) {
    "use strict";
    _inherits(SwcSubClass, SwcSuperClass);
    var _super = _create_super(SwcSubClass);
    function SwcSubClass() {
        _class_call_check(this, SwcSubClass);
        var _this;
        return _possible_constructor_return(_this);
    }
    return SwcSubClass;
}(SwcSuperClass);

var TsSuperClass = /** @class */ (function () {
    function TsSuperClass() {
    }
    return TsSuperClass;
}());
var TsSubClass = /** @class */ (function (_super) {
    __extends(TsSubClass, _super);
    function TsSubClass() {
        var _this = this;
        return _this;
    }
    return TsSubClass;
}(TsSuperClass));
`,
  `
var BabelSuperClass = /*#__PURE__*/_createClass(function BabelSuperClass() {
    _classCallCheck(this, BabelSuperClass);
});

class BabelSubClass extends BabelSuperClass {
    constructor() {
        var _this;
        _classCallCheck(this, BabelSubClass);
        return _possibleConstructorReturn(_this);
    }
}

var BabelSuperClass2 = /*#__PURE__*/_createClass(function BabelSuperClass2() {
    _classCallCheck(this, BabelSuperClass2);
});

class BabelSubClass2 extends BabelSuperClass2 {
    constructor() {
        var _this;
        _classCallCheck(this, BabelSubClass2);
        return _possibleConstructorReturn(_this);
    }
}

var SwcSuperClass = function SwcSuperClass() {
    "use strict";
    _class_call_check(this, SwcSuperClass);
};

class SwcSubClass extends SwcSuperClass {
    constructor() {
        _class_call_check(this, SwcSubClass);
        var _this;
        return _possible_constructor_return(_this);
    }
}

class TsSuperClass {}

class TsSubClass extends TsSuperClass {
    constructor() {
        var _this = this;
        return _this;
    }
}
`)

inlineTest.todo('ultimate class declaration',
  `
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
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g;
    return g = { next: verb(0), "throw": verb(1), "return": verb(2) }, typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
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
var __classPrivateFieldGet = (this && this.__classPrivateFieldGet) || function (receiver, state, kind, f) {
    if (kind === "a" && !f) throw new TypeError("Private accessor was defined without a getter");
    if (typeof state === "function" ? receiver !== state || !f : !state.has(receiver)) throw new TypeError("Cannot read private member from an object whose class did not declare it");
    return kind === "m" ? f : kind === "a" ? f.call(receiver) : f ? f.value : state.get(receiver);
};
var __classPrivateFieldSet = (this && this.__classPrivateFieldSet) || function (receiver, state, value, kind, f) {
    if (kind === "m") throw new TypeError("Private method is not writable");
    if (kind === "a" && !f) throw new TypeError("Private accessor was defined without a setter");
    if (typeof state === "function" ? receiver !== state || !f : !state.has(receiver)) throw new TypeError("Cannot write private member to an object whose class did not declare it");
    return (kind === "a" ? f.call(receiver, value) : f ? f.value = value : state.set(receiver, value)), value;
};
var AdvancedClass = /** @class */ (function () {
    function AdvancedClass(value) {
        _AdvancedClass_instances.add(this);
        this.publicInstanceField = "I'm public instance field";
        _AdvancedClass_privateInstanceField.set(this, "I'm private instance field");
        this.publicInstanceField = value;
    }
    Object.defineProperty(AdvancedClass.prototype, "readPrivateField", {
        // Getter
        get: function () {
            return __classPrivateFieldGet(this, _AdvancedClass_privateInstanceField, "f");
        },
        enumerable: false,
        configurable: true
    });
    Object.defineProperty(AdvancedClass.prototype, "modifyPrivateField", {
        // Setter
        set: function (value) {
            __classPrivateFieldSet(this, _AdvancedClass_privateInstanceField, value, "f");
        },
        enumerable: false,
        configurable: true
    });
    AdvancedClass.prototype.instanceMethod = function () {
        return 'This is an instance method.';
    };
    AdvancedClass.prototype.asyncInstanceMethod = function () {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_b) {
                return [2 /*return*/, 'This is an async instance method.'];
            });
        });
    };
    AdvancedClass.prototype.generatorInstanceMethod = function () {
        return __generator(this, function (_b) {
            switch (_b.label) {
                case 0: return [4 /*yield*/, 'This is a generator instance method.'];
                case 1:
                    _b.sent();
                    return [2 /*return*/];
            }
        });
    };
    AdvancedClass.staticMethod = function () {
        return 'This is a static method.';
    };
    AdvancedClass.asyncStaticMethod = function () {
        return __awaiter(this, void 0, void 0, function () {
            return __generator(this, function (_b) {
                return [2 /*return*/, 'This is an async static method.'];
            });
        });
    };
    AdvancedClass.generatorStaticMethod = function () {
        return __generator(this, function (_b) {
            switch (_b.label) {
                case 0: return [4 /*yield*/, 'This is a generator static method.'];
                case 1:
                    _b.sent();
                    return [2 /*return*/];
            }
        });
    };
    var _AdvancedClass_instances, _a, _AdvancedClass_privateInstanceField, _AdvancedClass_privateStaticField, _AdvancedClass_privateInstanceMethod, _AdvancedClass_privateStaticMethod;
    _a = AdvancedClass, _AdvancedClass_privateInstanceField = new WeakMap(), _AdvancedClass_instances = new WeakSet(), _AdvancedClass_privateInstanceMethod = function _AdvancedClass_privateInstanceMethod() {
        return 'This is a private instance method.';
    }, _AdvancedClass_privateStaticMethod = function _AdvancedClass_privateStaticMethod() {
        return 'This is a private static method.';
    };
    AdvancedClass.publicStaticField = "I'm public static field";
    _AdvancedClass_privateStaticField = { value: "I'm private static field" };
    return AdvancedClass;
}());
`,
  `
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
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g;
    return g = { next: verb(0), "throw": verb(1), "return": verb(2) }, typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
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
var __classPrivateFieldGet = (this && this.__classPrivateFieldGet) || function (receiver, state, kind, f) {
    if (kind === "a" && !f) throw new TypeError("Private accessor was defined without a getter");
    if (typeof state === "function" ? receiver !== state || !f : !state.has(receiver)) throw new TypeError("Cannot read private member from an object whose class did not declare it");
    return kind === "m" ? f : kind === "a" ? f.call(receiver) : f ? f.value : state.get(receiver);
};
var __classPrivateFieldSet = (this && this.__classPrivateFieldSet) || function (receiver, state, value, kind, f) {
    if (kind === "m") throw new TypeError("Private method is not writable");
    if (kind === "a" && !f) throw new TypeError("Private accessor was defined without a setter");
    if (typeof state === "function" ? receiver !== state || !f : !state.has(receiver)) throw new TypeError("Cannot write private member to an object whose class did not declare it");
    return (kind === "a" ? f.call(receiver, value) : f ? f.value = value : state.set(receiver, value)), value;
};
class AdvancedClass {
    constructor(value) {
        this.publicInstanceField = "I'm public instance field";
        this.publicInstanceField = value;
    }

    get readPrivateField() {
        return this.#privateInstanceField;
    }

    set modifyPrivateField(value) {
        this.#privateInstanceField = value;
    }

    instanceMethod() {
        return 'This is an instance method.';
    }

    #privateInstanceMethod() {
        return 'This is a private instance method.';
    }

    async asyncInstanceMethod() {
        return 'This is an async instance method.';
    }

    *generatorInstanceMethod() {
        yield 'This is a generator instance method.';
    }

    static staticMethod() {
        return 'This is a static method.';
    }

    static #privateStaticMethod() {
        return 'This is a private static method.';
    }

    static async asyncStaticMethod() {
        return 'This is an async static method.';
    }

    static *generatorStaticMethod() {
        yield 'This is a generator static method.';
    }
}`,
)

inlineTest('should not convert to class',
  `
var A = (function (p) {
    function A() {
        console.log(p);
    }
    return A;
}(p));
`,
  `
var A = (function (p) {
    function A() {
        console.log(p);
    }
    return A;
}(p));
`,
)
