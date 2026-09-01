export interface Example {
  value: string;
  label: string;
  source: string;
}

const BABEL_EXAMPLE = `\
"use strict";
var _classCallCheck = require("@babel/runtime/helpers/classCallCheck");
var _createClass = require("@babel/runtime/helpers/createClass");
var _asyncToGenerator = require("@babel/runtime/helpers/asyncToGenerator");
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
var React = require("react");

function formatResult(a) {
    var b = a.label;
    var c = a.score;
    var d = a.meta;
    var _d;
    var e = (_d = d === null || d === void 0 ? void 0 : d.tag) !== null && _d !== void 0 ? _d : "none";
    return "[".concat(e, "] ").concat(b, ": ").concat(Math.pow(c, 2));
}

var d = function(e) {
    var t = e.children, n = e.className;
    var h = _slicedToArray((0, React.useState)(""), 2);
    var g = h[0], y = h[1];
    var z = (0, React.useRef)(null);
    return ((0, React.useEffect)(function() {
        y(n);
    }, [n]),
    React.createElement("div", { className: "".concat(n, " ").concat(g), ref: z }, t));
};
d.displayName = "StatusPanel";

function processAll(a) {
    var b = [];
    for (var i = 0, c = a; i < c.length; i++) {
        var d = c[i];
        var _d;
        var e = (_d = d === null || d === void 0 ? void 0 : d.score) !== null && _d !== void 0 ? _d : 0;
        e > 0 && b.push(formatResult(d));
    }
    return b;
}

var summarize = function(a) {
    var b = a.filter(function(c) { return c.ok === !0; });
    var c = b.map(function(d) {
        var e = d.data, f = d.ok;
        return { data: e, status: f ? "pass" : "fail" };
    });
    console.log("Done: ".concat(c.length, "/").concat(a.length, " passed"));
    return c;
};

var TaskRunner = function() {
    function TaskRunner(a) {
        _classCallCheck(this, TaskRunner);
        var _a;
        this.name = (_a = a) !== null && _a !== void 0 ? _a : "default";
        this.tasks = [];
    }
    return _createClass(TaskRunner, [{
        key: "add",
        value: function(a) {
            var b = arguments.length > 1 && arguments[1] !== void 0 ? arguments[1] : 1;
            this.tasks.push({ name: a, priority: b, ts: Date.now() });
            console.log("[".concat(this.name, "] Added: ").concat(a, " (p=").concat(b, ")"));
        }
    }, {
        key: "run",
        value: function(a) {
            return _asyncToGenerator(regeneratorRuntime.mark(function _callee() {
                var response, data;
                return regeneratorRuntime.wrap(function _callee$(_context) {
                    while (1) switch (_context.prev = _context.next) {
                        case 0:
                            _context.prev = 0;
                            _context.next = 3;
                            return fetch("/api/tasks/".concat(a.name));
                        case 3:
                            response = _context.sent;
                            _context.next = 6;
                            return response.json();
                        case 6:
                            data = _context.sent;
                            return _context.abrupt("return", { ok: !0, data: data });
                        case 8:
                            _context.prev = 8;
                            _context.t0 = _context["catch"](0);
                            return _context.abrupt("return", { ok: !1, error: "Task ".concat(a.name, " failed") });
                        case 11:
                        case "end":
                            return _context.stop();
                    }
                }, _callee, null, [[0, 8]]);
            }))();
        }
    }]);
}();

var runner = new TaskRunner("demo");
runner.add("deploy", 2);
console.log(processAll(runner.tasks));
console.log(summarize([{ ok: !0, data: { label: "test", score: 5 } }]));
`;

const TYPESCRIPT_EXAMPLE = `\
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.loadTheme = exports.Status = void 0;
var tslib_1 = require("tslib");
var Status;
(function (Status) {
    Status[Status["Idle"] = 0] = "Idle";
    Status[Status["Loading"] = 1] = "Loading";
    Status[Status["Done"] = 2] = "Done";
})(Status = exports.Status || (exports.Status = {}));
var ThemeStore = /** @class */ (function () {
    function ThemeStore(name) {
        this.name = name;
        this.status = Status.Idle;
        this.listeners = [];
    }
    ThemeStore.prototype.subscribe = function (listener) {
        var _this = this;
        this.listeners.push(listener);
        return function () {
            _this.listeners = _this.listeners.filter(function (l) { return l !== listener; });
        };
    };
    ThemeStore.prototype.notify = function () {
        var _a;
        for (var _i = 0, _b = this.listeners; _i < _b.length; _i++) {
            var listener = _b[_i];
            listener((_a = this.name) !== null && _a !== void 0 ? _a : "default");
        }
    };
    return ThemeStore;
}());
function loadTheme(name) {
    return tslib_1.__awaiter(this, void 0, void 0, function () {
        var res, data, extras;
        return tslib_1.__generator(this, function (_a) {
            switch (_a.label) {
                case 0: return [4 /*yield*/, fetch("/themes/" + name + ".json")];
                case 1:
                    res = _a.sent();
                    return [4 /*yield*/, res.json()];
                case 2:
                    data = _a.sent();
                    extras = tslib_1.__spreadArray(["base"], data.tokens, true);
                    return [2 /*return*/, tslib_1.__assign(tslib_1.__assign({}, data), { tokens: extras, status: Status.Done })];
            }
        });
    });
}
exports.loadTheme = loadTheme;
`;

const TERSER_EXAMPLE = `\
"use strict";function normalize(e,t){void 0===t&&(t={});var n=t.limit,r=void 0===n?20:n,i=t.strict,o=void 0!==i&&i;return e&&"list"===e.kind?(o&&(r=Math.min(r,10)),e.items.filter(function(e){return null!=e.owner&&!1!==e.visible}).slice(0,r)):[]}function summarize(e){var t,n=normalize(e,{strict:!0}),r=0;for(t=0;t<n.length;t++)r+=null!=n[t].score?n[t].score:0;return n.length?(console.log("kept "+n.length+" of "+e.items.length),{ok:!0,total:r,average:r/n.length}):{ok:!1,total:0,average:void 0}}var report=function(e){var t=summarize(e);return t.ok?"total="+t.total+" avg="+t.average.toFixed(2):"empty"};!function(){var e={kind:"list",items:[{owner:"a",score:3,visible:!0},{owner:null,score:9},{owner:"b",score:5,visible:!0}]};console.log(report(e))}();
`;

const INTEROP_EXAMPLE = `\
"use strict";
Object.defineProperty(exports, "__esModule", { value: !0 });
exports.loadProfile = void 0;
var _api = _interopRequireDefault(require("./api"));
function _interopRequireDefault(e) { return e && e.__esModule ? e : { default: e }; }
function _asyncToGenerator(e) { return function () { var t = this, r = arguments; return new Promise(function (n, o) { var a = e.apply(t, r); function i(e) { c(a, n, o, i, u, "next", e); } function u(e) { c(a, n, o, i, u, "throw", e); } i(void 0); }); }; }
function c(e, t, r, n, o, a, i) { try { var u = e[a](i), c = u.value; } catch (e) { return void r(e); } u.done ? t(c) : Promise.resolve(c).then(n, o); }
var loadProfile = function () {
    var e = _asyncToGenerator(function* (e) {
        var t = yield _api.default.fetchUser(e), r = null != t.name ? t.name : "anonymous";
        return { name: r, avatar: null == t.profile ? void 0 : t.profile.avatar };
    });
    return function (t) { return e.apply(this, arguments); };
}();
exports.loadProfile = loadProfile;
`;

export const EXAMPLES: Example[] = [
  {
    value: "babel",
    label: "Babel: classes, async, hooks",
    source: BABEL_EXAMPLE,
  },
  {
    value: "typescript",
    label: "TypeScript: enum, class, __awaiter",
    source: TYPESCRIPT_EXAMPLE,
  },
  {
    value: "terser",
    label: "Terser: minifier artifacts",
    source: TERSER_EXAMPLE,
  },
  {
    value: "interop",
    label: "CommonJS interop → ESM",
    source: INTEROP_EXAMPLE,
  },
];

export const DEFAULT_EXAMPLE = EXAMPLES[0].source;
