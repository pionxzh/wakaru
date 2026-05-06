"use strict";
var r = require(7462);
var o = require(6854);

// Default parameters
function _j() {
    var a = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : "Info";
    var b = arguments.length > 1 ? arguments[1] : undefined;
    // Template literal
    alert("[" + a + "] Message from " + b);
}

// Rename component based on the displayName
var d = function (e) {
    // Object destructuring
    var t = e.children, n = e.className, c = e.visible, f = e.name;

    // Array destructuring
    var p = (0, r.useState)(""), h = (0, o.Z)(p, 2);
    var g = h[0], y = h[1];
    // Rename x to setW to follow the naming convention
    var b = (0, r.useState)(c), v = (0, o.Z)(b, 2);
    var w = v[0], x = v[1];
    // Rename it with Ref suffix
    var z = (0, r.useRef)(null);

    // Split sequence expression
    // Fix indirect call
    return ((0, r.useEffect)(
        function () {
            // Flip comparison + Boolean conversion
            var e = !0 == c ? "enter" : "leave";

            // Conditions to logical expression
            c && !w && x(!0);

            // For loop to while loop
            for (; i < 10;) console.log(i);

            var _e;
            // Optional chaining
            if ((_e = e) !== null && _e !== void 0 && (_e = _e.animation) !== null && _e !== void 0 && _e.enabled) {
                var n = setTimeout(function () {
                    // Template literal
                    y("".concat(f, "-").concat(e, " ").concat(f, "-").concat(e, "-active"));
                    clearTimeout(n);
                }, 1e3);

                return function () {
                    clearTimeout(n);
                };
            }
        },
        [c, w]
    ),
    r.createElement("div", { className: "".concat(n, " ").concat(g), ref: z }, t));
}
// Rename component to CssTransition
d.displayName = "CssTransition";
