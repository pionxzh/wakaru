export const DEFAULT_EXAMPLE = `\
function formatUser(a) {
    var b = arguments.length > 1 && arguments[1] !== void 0 ? arguments[1] : "en";
    var _a$profile;
    var name = (_a$profile = a === null || a === void 0 ? void 0 : a.profile) !== null && _a$profile !== void 0 ? _a$profile : "Anonymous";
    var greeting = "Hello, " + name + "! Your locale is " + b + ".";
    var isActive = !!a && !1 !== a.active;
    return { greeting: greeting, isActive: isActive };
}

var processItems = function (items) {
    var _ref = items[0];
    var head = _ref.id;
    var rest = items.slice(1);
    var result = rest.map(function (item) { return item.value * 2; });
    var total = result.reduce(function (sum, val) { return sum + val; }, 0);
    var config = items[0];
    var x = config.enabled;
    var y = config.label;
    var z = config.threshold;
    var msg = x !== null && x !== void 0 ? x : !1;
    console.log("Processing " + rest.length + " items, total: " + total);
    return { head: head, msg: msg, label: y, threshold: z };
};

function _j() {
    var a = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : "Info";
    var b = arguments.length > 1 ? arguments[1] : undefined;
    alert("[" + a + "] Message from " + b);
}

var d = function (e) {
    var t = e.children, n = e.className, c = e.visible, f = e.name;

    return (useEffect(
        function () {
            var e = !0 == c ? "enter" : "leave";
            c && !w && setW(!0);

            for (; i < 10;) console.log(i);

            var _e;
            if ((_e = e) !== null && _e !== void 0 && (_e = _e.animation) !== null && _e !== void 0 && _e.enabled) {
                var n = setTimeout(function () {
                    setClass("".concat(f, "-").concat(e, " ").concat(f, "-").concat(e, "-active"));
                    clearTimeout(n);
                }, 1e3);

                return function () {
                    clearTimeout(n);
                };
            }
        },
        [c, w]
    ),
    createElement("div", { className: "".concat(n, " ").concat(g), ref: z }, t));
};
d.displayName = "CssTransition";
`;
