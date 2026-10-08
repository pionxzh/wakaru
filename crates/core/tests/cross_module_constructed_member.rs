//! Cross-module constructor suffixes keep an object method constructible.
//!
//! Names and paths here are synthetic. The bundle suffix set is names only:
//! a false match keeps a function expression.

use wakaru_core::driver::test_support::{unpack_files, UnpackInput};
use wakaru_core::rules::RewriteLevel;
use wakaru_core::DecompileOptions;

fn unpack(files: &[(&str, &str)], emit_source_map: bool) -> Vec<(String, String)> {
    unpack_at(files, emit_source_map, RewriteLevel::Standard)
}

fn unpack_at(
    files: &[(&str, &str)],
    emit_source_map: bool,
    level: RewriteLevel,
) -> Vec<(String, String)> {
    let inputs = files
        .iter()
        .map(|(filename, source)| UnpackInput {
            filename: (*filename).to_string(),
            source: (*source).to_string(),
        })
        .collect();
    let output = unpack_files(
        inputs,
        DecompileOptions {
            emit_source_map,
            level,
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    output.modules
}

fn code<'a>(modules: &'a [(String, String)], name: &str) -> &'a str {
    modules
        .iter()
        .find(|(filename, _)| filename == name || filename.ends_with(&format!("/{name}")))
        .map(|(_, source)| source.as_str())
        .unwrap_or_else(|| {
            let names = modules
                .iter()
                .map(|(filename, _)| filename.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            panic!("missing {name} in {names}");
        })
}

fn assert_keeps_function(source: &str, name: &str) {
    let function_property = format!("{name}: function");
    assert!(
        source.contains(&function_property),
        "{name} must stay a function expression:\n{source}"
    );
}

fn assert_method_shorthand(source: &str, name: &str) {
    let function_property = format!("{name}: function");
    assert!(
        !source.contains(&function_property),
        "{name} must still use method shorthand:\n{source}"
    );
    let method = format!("{name}()");
    let method_spaced = format!("{name} (");
    assert!(
        source.contains(&method) || source.contains(&method_spaced),
        "{name} shorthand missing:\n{source}"
    );
}

const GADGET_DEF: &str = r#"
var ns = { group: {} };
function call(props) { return props; }
ns.group.Gadget = call({
    init: function (a, b) {
        this.a = a;
        var cb = function () { return 1; };
        this.nested = { ping: function () { return 2; } };
        return cb;
    },
    other: function () { return 3; }
});
var helper = function () { return 4; };
void helper;
export { ns };
"#;

#[test]
fn destructured_member_stays_constructible_across_modules() {
    // shape: wild-observed
    let modules = unpack(
        &[
            ("gadget-def.js", GADGET_DEF),
            (
                "gadget-use.js",
                r#"
import { ns } from "./gadget-def.js";
const { Gadget } = ns.group;
new Gadget.init(1, 2);
"#,
            ),
        ],
        false,
    );
    let def = code(&modules, "gadget-def.js");
    assert_keeps_function(def, "init");
    assert_method_shorthand(def, "other");
    assert_method_shorthand(def, "ping");
    assert!(
        def.contains("cb = () =>") || def.contains("cb = ()=>"),
        "callback inside the kept function must still become an arrow:\n{def}"
    );
    assert!(
        def.contains("helper = () =>") || def.contains("helper = ()=>"),
        "unrelated function must still become an arrow:\n{def}"
    );
}

#[test]
fn destructured_member_stays_constructible_with_source_map() {
    // shape: wild-observed
    let modules = unpack(
        &[
            ("gadget-def.js", GADGET_DEF),
            (
                "gadget-use.js",
                r#"
import { ns } from "./gadget-def.js";
const { Gadget } = ns.group;
new Gadget.init(1, 2);
"#,
            ),
        ],
        true,
    );
    let def = code(&modules, "gadget-def.js");
    assert_keeps_function(def, "init");
    assert_method_shorthand(def, "other");
}

#[test]
fn one_segment_new_does_not_invent_a_parent() {
    let modules = unpack(
        &[
            (
                "algo-def.js",
                r#"
var holder = {};
function call(props) { return props; }
holder.Algo = call({
    init: function (a, b) { this.a = a; },
    reset: function () { return 0; }
});
export { holder };
"#,
            ),
            (
                "algo-root.js",
                r#"
function Algo() {}
new Algo.init(1, 2);
export { Algo };
"#,
            ),
        ],
        false,
    );
    let def = code(&modules, "algo-def.js");
    assert_method_shorthand(def, "init");
    assert_method_shorthand(def, "reset");
}

#[test]
fn binding_alias_chain_stays_constructible_across_modules() {
    // shape: wild-observed
    let modules = unpack(
        &[
            ("gadget-def.js", GADGET_DEF),
            (
                "gadget-alias.js",
                r#"
import { ns } from "./gadget-def.js";
var c = ns.group.Gadget;
var d = c;
new d.init(1, 2);
"#,
            ),
        ],
        false,
    );
    assert_keeps_function(code(&modules, "gadget-def.js"), "init");
    assert_method_shorthand(code(&modules, "gadget-def.js"), "other");
}

#[test]
fn member_new_stays_constructible_across_modules() {
    // shape: wild-observed
    let modules = unpack(
        &[
            (
                "algo-def.js",
                r#"
var holder = {};
function call(props) { return props; }
holder.Algo = call({
    init: function (a, b) { this.a = a; },
    reset: function () { return 0; }
});
export { holder };
"#,
            ),
            (
                "algo-use.js",
                r#"
import { holder } from "./algo-def.js";
new holder.Algo.init(1, 2);
"#,
            ),
        ],
        false,
    );
    let def = code(&modules, "algo-def.js");
    assert_keeps_function(def, "init");
    assert_method_shorthand(def, "reset");
}

#[test]
fn conditional_callee_stays_constructible_across_modules() {
    // shape: wild-observed
    let modules = unpack(
        &[
            (
                "algo-def.js",
                r#"
var holder = {};
function call(props) { return props; }
holder.Algo = call({
    init: function (a, b) { this.a = a; },
    reset: function () { return 0; }
});
export { holder };
"#,
            ),
            (
                "algo-cond.js",
                r#"
import { holder } from "./algo-def.js";
var flag = true;
function Fallback() {}
new (flag ? holder.Algo.init : Fallback)(1, 2);
"#,
            ),
        ],
        false,
    );
    assert_keeps_function(code(&modules, "algo-def.js"), "init");
}

#[test]
fn different_parent_still_uses_method_shorthand() {
    let modules = unpack(
        &[
            (
                "algo-def.js",
                r#"
var holder = {};
function call(props) { return props; }
holder.Algo = call({
    init: function (a, b) { this.a = a; }
});
export { holder };
"#,
            ),
            (
                "algo-use.js",
                r#"
import { holder } from "./algo-def.js";
new holder.Algo.init(1, 2);
"#,
            ),
            (
                "other-def.js",
                r#"
var other = {};
function call(props) { return props; }
other.Other = call({
    init: function () {},
    side: function () { return 1; }
});
export { other };
"#,
            ),
        ],
        false,
    );
    assert_method_shorthand(code(&modules, "other-def.js"), "init");
    assert_method_shorthand(code(&modules, "other-def.js"), "side");
}

#[test]
fn prototype_parent_still_uses_method_shorthand() {
    let modules = unpack(
        &[
            (
                "proto-use.js",
                r#"
function A() {}
new A.prototype.init();
export { A };
"#,
            ),
            (
                "proto-def.js",
                r#"
var B = function () {};
B.prototype = {
    init: function () {}
};
export { B };
"#,
            ),
        ],
        false,
    );
    assert_method_shorthand(code(&modules, "proto-def.js"), "init");
}

#[test]
fn no_construct_use_still_uses_method_shorthand() {
    let modules = unpack(
        &[
            (
                "quiet-def.js",
                r#"
var holder = {};
function call(props) { return props; }
holder.Algo = call({
    init: function (a, b) { this.a = a; },
    reset: function () { return 0; }
});
export { holder };
"#,
            ),
            (
                "quiet-use.js",
                r#"
import { holder } from "./quiet-def.js";
holder.Algo.init(1, 2);
"#,
            ),
        ],
        false,
    );
    let def = code(&modules, "quiet-def.js");
    assert_method_shorthand(def, "init");
    assert_method_shorthand(def, "reset");
}

#[test]
fn ident_spread_and_computed_neighbors_still_shorthand() {
    let modules = unpack(
        &[
            (
                "loose-def.js",
                r#"
var holder = {};
var existing = {
    init: function () {},
    side: function () { return 1; }
};
var items = [{ init: function () {}, tail: function () { return 2; } }];
function call(props) { return props; }
holder.Algo = call(existing);
holder.Spread = call(...items);
holder.Computed = call({
    init: function () {},
    ["nope"]: function () { return 3; },
    neighbor: function () { return 4; }
});
export { holder };
"#,
            ),
            (
                "loose-use.js",
                r#"
import { holder } from "./loose-def.js";
new holder.Algo.init();
new holder.Spread.init();
new holder.Computed.init();
"#,
            ),
        ],
        false,
    );
    let def = code(&modules, "loose-def.js");
    assert_method_shorthand(def, "side");
    assert_method_shorthand(def, "tail");
    assert_method_shorthand(def, "neighbor");
}

#[test]
fn descriptor_value_still_uses_method_shorthand() {
    let modules = unpack(
        &[
            (
                "algo-def.js",
                r#"
var holder = {};
function call(props) { return props; }
holder.Algo = call({
    init: function (a, b) { this.a = a; }
});
export { holder };
"#,
            ),
            (
                "algo-use.js",
                r#"
import { holder } from "./algo-def.js";
new holder.Algo.init(1, 2);
"#,
            ),
            (
                "desc.js",
                r#"
var desc = {
    key: "make",
    value: function () { return 1; }
};
export { desc };
"#,
            ),
        ],
        false,
    );
    assert_method_shorthand(code(&modules, "desc.js"), "value");
}

#[test]
fn async_property_stays_async_function() {
    let modules = unpack(
        &[
            (
                "async-def.js",
                r#"
var holder = {};
function call(props) { return props; }
holder.Algo = call({
    init: async function (a, b) { this.a = a; }
});
export { holder };
"#,
            ),
            (
                "async-use.js",
                r#"
import { holder } from "./async-def.js";
new holder.Algo.init(1, 2);
"#,
            ),
        ],
        false,
    );
    let def = code(&modules, "async-def.js");
    assert!(
        def.contains("async function"),
        "async function expression must stay async:\n{def}"
    );
    assert!(
        !def.contains("async init(") && !def.contains("async init ("),
        "async property must not become a method:\n{def}"
    );
}
