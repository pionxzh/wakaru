//! A proven createClass helper installs an anonymous descriptor `value` onto
//! the constructor. When the same module constructs that member, the `value`
//! stays a function expression.

use wakaru_core::{decompile, DecompileOptions};

fn decompile_module(source: &str) -> String {
    decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code
}

fn assert_keeps_value_function(source: &str, marker: &str) {
    let window = window_around(source, marker);
    assert!(
        window.contains("value: function"),
        "descriptor value around {marker:?} must stay a function expression:\n{source}"
    );
}

fn assert_value_shorthand(source: &str, marker: &str) {
    let window = window_around(source, marker);
    assert!(
        !window.contains("value: function"),
        "descriptor value around {marker:?} must stay method shorthand:\n{source}"
    );
    assert!(
        window.contains("value()") || window.contains("value ("),
        "shorthand missing around {marker:?}:\n{source}"
    );
}

fn window_around<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing {marker:?}:\n{source}"));
    let from = source[..start].rfind("key:").unwrap_or(0);
    let to = source[start..]
        .find("key:")
        .map(|index| start + index)
        .unwrap_or(source.len());
    &source[from..to]
}

/// shape: producer terser@5.51.2 module:true, compress:{defaults:true, unused:false}, mangle:true, format:{comments:false}
/// Legal ES5 source. The top-level helper body is what
/// `@babel/core@7.29.7` + `@babel/preset-env@7.29.7` (`targets: { ie: "11" }`,
/// `modules: false`) emits for `class Empty {}`. The wrapper returns the
/// constructor (`return n(...), t`). A `{ key: "tag", value: 1 }` descriptor
/// makes UnEs6Class reject the class in the source. A second helper call keeps
/// the helper from being inlined into a temporary array.
const TERSER_STATIC_NEW: &str = r#"function t(e){return t="function"==typeof Symbol&&"symbol"==typeof Symbol.iterator?function(t){return typeof t}:function(t){return t&&"function"==typeof Symbol&&t.constructor===Symbol&&t!==Symbol.prototype?"symbol":typeof t},t(e)}function e(t,e){for(var n=0;n<e.length;n++){var o=e[n];o.enumerable=o.enumerable||!1,o.configurable=!0,"value"in o&&(o.writable=!0),Object.defineProperty(t,r(o.key),o)}}function n(t,n,r){return n&&e(t.prototype,n),r&&e(t,r),Object.defineProperty(t,"prototype",{writable:!1}),t}function r(e){var n=o(e,"string");return"symbol"==t(n)?n:n+""}function o(e,n){if("object"!=t(e)||!e)return e;var r;if("undefined"!=typeof Symbol&&void 0!==(r=e[Symbol.toPrimitive])){var o=r.call(e,n||"default");if("object"!=t(o))return o;throw new TypeError("@@toPrimitive must return a primitive value.")}return("string"===n?String:Number)(e)}var u=function(){function t(){}return n(t,[{key:"ping",value:function(){return 1}}],[{key:"tag",value:1},{key:"make",value:function(t){var e=function(t){return t};return this.slot=e,this.slot(t)}},{key:"idle",value:function(){return 3}}]),t}(),i;function f(){}new(Math.random()?u.make:f);var l=function(){function t(){}return n(t,null,[{key:"hold",value:function(){return 4}}]),t}();l.hold();"#;

/// shape: producer terser@5.51.2 module:true, compress:{defaults:true, unused:false}, mangle:true, format:{comments:false}
/// Same helper and ES5 source as `TERSER_STATIC_NEW`, except the construct
/// use is `new Outer.prototype.ping`.
const TERSER_PROTO_NEW: &str = r#"function t(n){return t="function"==typeof Symbol&&"symbol"==typeof Symbol.iterator?function(t){return typeof t}:function(t){return t&&"function"==typeof Symbol&&t.constructor===Symbol&&t!==Symbol.prototype?"symbol":typeof t},t(n)}function n(t,n){for(var e=0;e<n.length;e++){var o=n[e];o.enumerable=o.enumerable||!1,o.configurable=!0,"value"in o&&(o.writable=!0),Object.defineProperty(t,r(o.key),o)}}function e(t,e,r){return e&&n(t.prototype,e),r&&n(t,r),Object.defineProperty(t,"prototype",{writable:!1}),t}function r(n){var e=o(n,"string");return"symbol"==t(e)?e:e+""}function o(n,e){if("object"!=t(n)||!n)return n;var r;if("undefined"!=typeof Symbol&&void 0!==(r=n[Symbol.toPrimitive])){var o=r.call(n,e||"default");if("object"!=t(o))return o;throw new TypeError("@@toPrimitive must return a primitive value.")}return("string"===e?String:Number)(n)}var u=function(){function t(){}return e(t,[{key:"ping",value:function(){return 1}}],[{key:"tag",value:1},{key:"make",value:function(t){var n=function(t){return t};return this.slot=n,this.slot(t)}},{key:"idle",value:function(){return 3}}]),t}(),i=Math.random();function f(){}new u.prototype.ping;var l=function(){function t(){}return e(t,null,[{key:"hold",value:function(){return 4}}]),t}();l.hold();"#;

#[test]
fn constructed_static_descriptor_value_stays_function() {
    let output = decompile_module(TERSER_STATIC_NEW);
    assert_keeps_value_function(&output, "this.slot");
    assert_value_shorthand(&output, "return 1");
    assert_value_shorthand(&output, "return 3");
    assert!(
        output.contains("(t) => t") || output.contains("(t)=>t"),
        "callback inside the kept value must still become an arrow:\n{output}"
    );
    assert!(
        output.contains("static hold"),
        "a helper call UnEs6Class accepts stays a class static:\n{output}"
    );
}

#[test]
fn constructed_prototype_descriptor_value_stays_function() {
    let output = decompile_module(TERSER_PROTO_NEW);
    assert_keeps_value_function(&output, "return 1");
    assert_value_shorthand(&output, "this.slot");
    assert_value_shorthand(&output, "return 3");
}

const INSTALL_HELPER: &str = r#"
function install(Ctor, proto, stat) {
    if (proto) write(Ctor.prototype, proto);
    if (stat) write(Ctor, stat);
    return Ctor;
}
function write(target, props) {
    return target;
}
"#;

fn class_with_make(body: &str) -> String {
    format!(
        r#"
{helper}
var Outer = (function () {{
    function Inner() {{}}
    install(Inner, null, [
        {{ key: "make", value: function () {{ {body} }} }},
        {{ key: "tag", value: 1 }}
    ]);
    return Inner;
}})();
"#,
        helper = INSTALL_HELPER,
        body = body,
    )
}

#[test]
fn other_constructor_same_key_stays_shorthand() {
    let source = format!(
        "{}{}",
        class_with_make("return 1;"),
        r#"
var Other = (function () {
    function Inner() {}
    install(Inner, null, [
        { key: "make", value: function () { return 2; } },
        { key: "tag", value: 1 }
    ]);
    return Inner;
})();
new Other.make();
"#
    );
    let output = decompile_module(&source);
    assert_value_shorthand(&output, "return 1");
    assert_keeps_value_function(&output, "return 2");
}

#[test]
fn unproven_helper_name_still_uses_method_shorthand() {
    let source = r#"
function install(Ctor, proto, stat) {
    return Ctor;
}
var Outer = (function () {
    function Inner() {}
    install(Inner, null, [
        { key: "make", value: function () { return 1; } },
        { key: "tag", value: 1 }
    ]);
    return Inner;
})();
new Outer.make();
"#;
    assert_value_shorthand(&decompile_module(source), "return 1");
}

#[test]
fn async_descriptor_value_stays_async_function() {
    let source = class_with_make("return 1;").replace(
        "value: function () { return 1; }",
        "value: async function () { return 1; }",
    ) + "new Outer.make();";
    let output = decompile_module(&source);
    assert!(
        output.contains("value: async function"),
        "async descriptor value must stay async:\n{output}"
    );
    assert!(
        !output.contains("async value(") && !output.contains("async value ("),
        "async descriptor value must not become a method:\n{output}"
    );
}

#[test]
fn computed_spread_and_getter_do_not_freeze_siblings() {
    let source = format!(
        "{}{}",
        INSTALL_HELPER,
        r#"
var computed = "nope";
var extra = [{ key: "side", value: function () { return 8; } }];
var Outer = (function () {
    function Inner() {}
    install(Inner, null, [
        { key: "make", value: function () { return 1; } },
        { key: computed, value: function () { return 7; } },
        { key: "getty", get: function () { return 6; } },
        { key: "tag", value: 1 },
        ...extra
    ]);
    return Inner;
})();
new Outer.make();
"#
    );
    let output = decompile_module(&source);
    assert_keeps_value_function(&output, "return 1");
    assert_value_shorthand(&output, "return 7");
    assert_value_shorthand(&output, "return 8");
}

#[test]
fn descriptor_array_through_temp_stays_shorthand() {
    let source = format!(
        "{}{}",
        INSTALL_HELPER,
        r#"
var Outer = (function () {
    function Inner() {}
    var methods = [
        { key: "make", value: function () { return 1; } },
        { key: "tag", value: 1 }
    ];
    install(Inner, null, methods);
    return Inner;
})();
new Outer.make();
"#
    );
    assert_value_shorthand(&decompile_module(&source), "return 1");
}

#[test]
fn returned_helper_call_does_not_link_outer_new() {
    let source = format!(
        "{}{}",
        INSTALL_HELPER,
        r#"
var Outer = (function () {
    function Inner() {}
    return install(Inner, null, [
        { key: "make", value: function () { return 1; } },
        { key: "tag", value: 1 }
    ]);
})();
new Outer.make();
"#
    );
    assert_value_shorthand(&decompile_module(&source), "return 1");
}

#[test]
fn instance_new_does_not_keep_prototype_value() {
    let source = format!(
        "{}{}",
        INSTALL_HELPER,
        r#"
var Outer = (function () {
    function Inner() {}
    install(Inner, [
        { key: "ping", value: function () { return 1; } }
    ], [
        { key: "tag", value: 1 }
    ]);
    return Inner;
})();
var inst = new Outer();
new inst.ping();
"#
    );
    assert_value_shorthand(&decompile_module(&source), "return 1");
}

#[test]
fn nested_factory_helper_keeps_constructed_value() {
    let source = r#"
function factory() {
    function install(Ctor, proto, stat) {
        if (proto) write(Ctor.prototype, proto);
        if (stat) write(Ctor, stat);
        return Ctor;
    }
    function write(target, props) {
        return target;
    }
    var Outer = (function () {
        function Inner() {}
        install(Inner, null, [
            { key: "make", value: function () { return 1; } },
            { key: "idle", value: function () { return 3; } },
            { key: "tag", value: 1 }
        ]);
        return Inner;
    })();
    new Outer.make();
    return Outer;
}
void factory();
"#;
    let output = decompile_module(source);
    assert_keeps_value_function(&output, "return 1");
    assert_value_shorthand(&output, "return 3");
}

// A helper-shaped function that copies an object literal onto its result is
// not a descriptor array. `new Model.make` must keep the call-result link.
#[test]
fn helper_shaped_object_argument_keeps_call_result_member() {
    let source = r#"
function defineClass(Ctor, proto, statics) {
    if (proto) Object.assign(Ctor.prototype, proto);
    if (statics) Object.assign(Ctor, statics);
    return Ctor;
}
function Base() {}
var Model = defineClass(Base, null, {
    make: function (a) { this.a = a; }
});
new Model.make(1);
"#;
    let output = decompile_module(source);
    assert!(
        output.contains("make: function"),
        "object-literal member copied onto the call result must stay a function:\n{output}"
    );
}
