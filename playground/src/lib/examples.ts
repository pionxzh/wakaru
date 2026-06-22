export const DEFAULT_EXAMPLE = `\
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
