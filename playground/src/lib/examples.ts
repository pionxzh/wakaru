export const DEFAULT_EXAMPLE = `\
function _classCallCheck(a,b){if(!(a instanceof b))throw new TypeError("Cannot call a class as a function")}function _defineProperties(a,b){for(var c=0;c<b.length;c++){var d=b[c];d.enumerable=d.enumerable||!1,d.configurable=!0,"value"in d&&(d.writable=!0),Object.defineProperty(a,d.key,d)}}function _createClass(a,b,c){return b&&_defineProperties(a.prototype,b),c&&_defineProperties(a,c),a}

var Store = function() {
    function Store(a) {
        _classCallCheck(this, Store);
        this.name = a !== null && a !== void 0 ? a : "default";
        this.items = [];
    }
    return _createClass(Store, [{
        key: "add",
        value: function(a) {
            var b = arguments.length > 1 && arguments[1] !== void 0 ? arguments[1] : 1;
            this.items.push({ name: a, qty: b, added: Date.now() });
            console.log("[".concat(this.name, "] Added ").concat(b, "x ").concat(a));
        }
    }, {
        key: "find",
        value: function(a) {
            return this.items.filter(function(b) { return b.name === a; });
        }
    }, {
        key: "summary",
        value: function() {
            var a = this.items;
            var b = a.reduce(function(c, d) { return c + d.qty; }, 0);
            return { store: this.name, count: a.length, total: b };
        }
    }]);
}();

function getLabel(a) {
    var _a$meta;
    var b = (_a$meta = a === null || a === void 0 ? void 0 : a.meta) !== null && _a$meta !== void 0 ? _a$meta : {};
    var c = b.label;
    var d = b.priority;
    var e = c !== null && c !== void 0 ? c : "Untitled";
    var f = d !== null && d !== void 0 ? d : 0;
    return "".concat(e, " (priority: ").concat(f, ")");
}

function validate(a) {
    var _a;
    if ((_a = a) !== null && _a !== void 0 && (_a = _a.config) !== null && _a !== void 0 && _a.strict) {
        var rules = a.rules;
        for (var i = 0; i < rules.length; i++) {
            var rule = rules[i];
            if (rule.enabled === !1) console.warn("Rule " + rule.name + " is disabled");
        }
    }
}

var processAll = function(a) {
    var b = a.filter(function(c) { return c.qty > 0; });
    var c = b.map(function(d) {
        var e = d.name, f = d.qty;
        return { name: e, total: Math.pow(f, 2), label: getLabel(d) };
    });
    var d = c.reduce(function(e, f) { return e + f.total; }, 0);
    console.log("Processed " + c.length + " items, sum: " + d);
    return { items: c, sum: d };
};
`;
