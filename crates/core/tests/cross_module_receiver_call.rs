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
