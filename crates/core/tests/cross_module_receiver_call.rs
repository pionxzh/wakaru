//! A consumer's method call on a provider export keeps its receiver when the
//! provider's function reads `this`.
//!
//! In CommonJS, `t.describe()` passes the provider's `exports` object as
//! `this`. Unpack recovers `t` as a namespace import; the namespace object
//! still supplies the other exports to `this`, while a bare `describe()`
//! would call with `this` undefined.

use wakaru_core::driver::test_support::unpack;
use wakaru_core::{DecompileOptions, RewriteLevel};

// shape: producer webpack@5.111.1 mode production, target node,
// concatenateModules off, Terser; hand-written strict CommonJS source:
//   entry.js:    var provider = require("./provider.js");
//                globalThis.receiverReport = "receiver-entry " + provider.describe();
//   provider.js: exports.label = …; exports.size = 3;
//                exports.describe = function () { return this.label + "/" + this.size; };
const WEBPACK_METHOD_READS_THIS: &str = r#"(()=>{var e={97(e,r,i){"use strict";var t=i(240);globalThis.receiverReport="receiver-entry "+t.describe()},240(e,r){"use strict";r.label="receiver-provider",r.size=3,r.describe=function(){return this.label+"/"+this.size}}};const r={};globalThis.__artificialSink=function i(t){const s=r[t];if(void 0!==s)return s.exports;const o=r[t]={exports:{}};return e[t](o,o.exports,i),o.exports}(97)})();"#;

fn consumer(level: RewriteLevel) -> String {
    unpack(
        WEBPACK_METHOD_READS_THIS,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            level,
            ..Default::default()
        },
    )
    .expect("unpack should succeed")
    .modules
    .into_iter()
    .find(|(filename, _)| filename == "module-97.js")
    .map(|(_, code)| code)
    .expect("expected the consumer module")
}

#[test]
fn method_call_on_receiver_reading_export_keeps_namespace_receiver() {
    for level in [
        RewriteLevel::Minimal,
        RewriteLevel::Standard,
        RewriteLevel::Aggressive,
    ] {
        let code = consumer(level);
        assert!(
            code.contains("import * as t from \"./module-240.js\""),
            "{level:?}: the consumer must keep a namespace import:\n{code}"
        );
        assert!(
            code.contains("t.describe()"),
            "{level:?}: the call must keep the namespace as its receiver:\n{code}"
        );
    }
}

// shape: producer @babel/core@7.28.5 @babel/plugin-transform-async-to-generator@7.28.6
// (sourceType script, per file) then @rspack/core@1.7.12 mode production,
// target node, concatenateModules off, no minification; hand-written strict
// CommonJS source:
//   provider-decl.js: exports.describe = describe;
//                     async function describe() { return this.label + "/async"; }
//   provider-expr.js: exports.describe = async function () { return this.label + "/async"; };
//   entry.js:         Promise.all([decl.describe(), expr.describe()])
const RSPACK_BABEL_ASYNC_METHODS_READ_THIS: &str = r#"(() => {
var __webpack_modules__ = ({
97(__unused_rspack_module, __unused_rspack_exports, __webpack_require__) {
"use strict";


var decl = __webpack_require__(357);
var expr = __webpack_require__(158);
globalThis.receiverEntry = "receiver-async-entry";
globalThis.receiverReport = Promise.all([decl.describe(), expr.describe()]);

},
357(__unused_rspack_module, exports) {
"use strict";


function asyncGeneratorStep(n, t, e, r, o, a, c) { try { var i = n[a](c), u = i.value; } catch (n) { return void e(n); } i.done ? t(u) : Promise.resolve(u).then(r, o); }
function _asyncToGenerator(n) { return function () { var t = this, e = arguments; return new Promise(function (r, o) { var a = n.apply(t, e); function _next(n) { asyncGeneratorStep(a, r, o, _next, _throw, "next", n); } function _throw(n) { asyncGeneratorStep(a, r, o, _next, _throw, "throw", n); } _next(void 0); }); }; }
exports.label = "receiver-async-decl";
exports.describe = describe;
function describe() {
  return _describe.apply(this, arguments);
}
function _describe() {
  _describe = _asyncToGenerator(function* () {
    return this.label + "/async";
  });
  return _describe.apply(this, arguments);
}

},
158(__unused_rspack_module, exports) {
"use strict";


function asyncGeneratorStep(n, t, e, r, o, a, c) { try { var i = n[a](c), u = i.value; } catch (n) { return void e(n); } i.done ? t(u) : Promise.resolve(u).then(r, o); }
function _asyncToGenerator(n) { return function () { var t = this, e = arguments; return new Promise(function (r, o) { var a = n.apply(t, e); function _next(n) { asyncGeneratorStep(a, r, o, _next, _throw, "next", n); } function _throw(n) { asyncGeneratorStep(a, r, o, _next, _throw, "throw", n); } _next(void 0); }); }; }
exports.label = "receiver-async-expr";
exports.describe = _asyncToGenerator(function* () {
  return this.label + "/async";
});

},

});
// The module cache
var __webpack_module_cache__ = {};

// The require function
function __webpack_require__(moduleId) {

// Check if module is in cache
var cachedModule = __webpack_module_cache__[moduleId];
if (cachedModule !== undefined) {
return cachedModule.exports;
}
// Create a new module (and put it into the cache)
var module = (__webpack_module_cache__[moduleId] = {
exports: {}
});
// Execute the module function
__webpack_modules__[moduleId](module, module.exports, __webpack_require__);

// Return the exports of the module
return module.exports;

}

// webpack/runtime/rspack_version
(() => {
__webpack_require__.rv = () => ("1.7.12")
})();
// webpack/runtime/rspack_unique_id
(() => {
__webpack_require__.ruid = "bundler=rspack@1.7.12";
})();
var __webpack_exports__ = {};
// This entry needs to be wrapped in an IIFE because it needs to be isolated against other modules in the chunk.
(() => {
globalThis.__artificialSink = __webpack_require__(97);

})();

})()
;"#;

/// Babel lowers the declared async method to a wrapper that forwards its
/// receiver to the generator, and the function expression to an
/// `_asyncToGenerator(function* () { … })` call. Both generators read `this`.
#[test]
fn babel_async_methods_reading_this_keep_namespace_receiver() {
    for level in [
        RewriteLevel::Minimal,
        RewriteLevel::Standard,
        RewriteLevel::Aggressive,
    ] {
        let code = unpack(
            RSPACK_BABEL_ASYNC_METHODS_READ_THIS,
            DecompileOptions {
                filename: "bundle.js".to_string(),
                level,
                ..Default::default()
            },
        )
        .expect("unpack should succeed")
        .modules
        .into_iter()
        .find(|(filename, _)| filename == "module-97.js")
        .map(|(_, code)| code)
        .expect("expected the consumer module");
        for call in ["decl.describe()", "expr.describe()"] {
            assert!(
                code.contains(call),
                "{level:?}: `{call}` must keep the namespace as its receiver:\n{code}"
            );
        }
    }
}
