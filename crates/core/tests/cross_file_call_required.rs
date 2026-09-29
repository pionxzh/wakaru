//! Cross-file `.call` / `.apply` keeps the providing export callable.
//!
//! Names and paths here are synthetic. A recognized `extends` IIFE is not a
//! pin: Phase 2 turns that call into `super()`.

use wakaru_core::driver::test_support::{unpack_files, UnpackInput};
use wakaru_core::rules::RewriteLevel;
use wakaru_core::{DecompileOptions, UnpackWarningKind};

fn class_iife(export_name: &str) -> String {
    format!(
        r#"
var {export_name} = (function () {{
    function t() {{}}
    t.prototype.ping = function () {{ return 1; }};
    return t;
}})();
export {{ {export_name} }};
"#
    )
}

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
    // Every case here is one the super() prediction handles. A surviving call
    // to a predicted provider means the probe drifted from UnEs6Class.
    let mispredicted = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::CrossModuleClassCall)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(mispredicted.is_empty(), "{mispredicted:#?}");
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

fn assert_stays_function(source: &str, export_name: &str) {
    assert!(
        !declares_class(source, export_name),
        "{export_name} must stay a function:\n{source}"
    );
    assert!(
        !source.contains("super(") && !source.contains("super ("),
        "{export_name} must not gain super():\n{source}"
    );
}

fn declares_class(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with(&format!("class {name}"))
            || trimmed.starts_with(&format!("export class {name}"))
            || trimmed.starts_with(&format!("export default class {name}"))
    })
}

#[test]
fn cross_file_call_keeps_the_provider_a_function() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (Base) {
    function t() { return Base.call(this) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    let child_out = code(&modules, "child.js");
    assert_stays_function(base_out, "Foo");
    assert!(
        child_out.contains(".call"),
        "child must keep the call:\n{child_out}"
    );
    assert!(
        !child_out.contains("extends"),
        "child must not become class extends:\n{child_out}"
    );
}

#[test]
fn no_cross_file_call_still_becomes_a_class() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function () {
    function t() {}
    t.prototype.pong = function () { return Foo; };
    return t;
})();
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        base_out.contains("class Foo"),
        "uncalled provider should become a class:\n{base_out}"
    );
}

#[test]
fn same_module_extends_call_does_not_pin_the_parent() {
    let source = r#"
var Foo = (function () {
    function t() {}
    t.prototype.ping = function () { return 1; };
    return t;
})();
var Child = (function (_super) {
    __extends(t, _super);
    function t() { _super.call(this); }
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
export { Foo, Child };
"#;
    let modules = unpack(&[("same.js", source)], false);
    let out = &modules[0].1;
    assert!(
        out.contains("class Foo") && out.contains("class Child extends Foo"),
        "same-module extends should still recover both classes:\n{out}"
    );
}

#[test]
fn inherits_loose_without_call_does_not_pin() {
    let base = class_iife("Foo");
    let helpers = r#"
export function inheritsLoose(ctor, base) { ctor.prototype = Object.create(base.prototype); }
"#;
    let child = r#"
import { inheritsLoose } from "./helpers.js";
import { Foo } from "./base.js";
var Child = (function (Base) {
    function t() {}
    inheritsLoose(t, Base);
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(
        &[
            ("base.js", &base),
            ("helpers.js", helpers),
            ("child.js", child),
        ],
        false,
    );
    let base_out = code(&modules, "base.js");
    assert!(
        base_out.contains("class Foo"),
        "inheritsLoose alone must not pin Foo:\n{base_out}"
    );
}

#[test]
fn pin_one_export_does_not_pin_its_sibling() {
    let base = r#"
var Foo = (function () {
    function t() {}
    t.prototype.ping = function () { return 1; };
    return t;
})();
var Bar = (function () {
    function t() {}
    t.prototype.ping = function () { return 2; };
    return t;
})();
export { Foo, Bar };
"#;
    let child = r#"
import { Foo } from "./base.js";
Foo.call(this);
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert_stays_function(base_out, "Foo");
    assert!(
        base_out.contains("class Bar"),
        "Bar is not called and should become a class:\n{base_out}"
    );
}

#[test]
fn external_call_does_not_pin() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
Unknown.call(this);
export var seen = Foo;
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        base_out.contains("class Foo"),
        "an unresolved callee must not pin Foo:\n{base_out}"
    );
}

#[test]
fn plain_alias_is_not_followed() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
const alias = Foo;
alias.call(this);
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        base_out.contains("class Foo"),
        "a plain alias call must not pin Foo:\n{base_out}"
    );
}

#[test]
fn reexport_follows_the_definition_and_ignores_a_decoy() {
    let real = class_iife("Foo");
    let decoy = class_iife("Foo");
    let mid = r#"
export { Foo } from "./base.js";
"#;
    let child = r#"
import { Foo } from "./mid.js";
Foo.call(this);
"#;
    let modules = unpack(
        &[
            ("base.js", &decoy),
            ("pkg/base.js", &real),
            ("pkg/mid.js", mid),
            ("pkg/child.js", child),
        ],
        false,
    );
    assert_stays_function(code(&modules, "pkg/base.js"), "Foo");
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "the root decoy must still become a class:\n{}",
        code(&modules, "base.js")
    );
}

#[test]
fn reexport_cycle_stops() {
    let a = r#"export { Foo } from "./b.js";"#;
    let b = r#"export { Foo } from "./a.js";"#;
    let child = r#"
import { Foo } from "./a.js";
Foo.call(this);
"#;
    let modules = unpack(&[("a.js", a), ("b.js", b), ("child.js", child)], false);
    assert!(code(&modules, "child.js").contains("Foo"));
}

#[test]
fn import_rename_iife_param_namespace_and_call_apply_pin() {
    let base = class_iife("Foo");
    let renamed = r#"
import { Foo as Local } from "./base.js";
Local.call(this);
"#;
    let param = r#"
import { Foo } from "./base.js";
(function (Base) { Base.call(this); })(Foo);
"#;
    let namespace = r#"
import * as ns from "./base.js";
ns.Foo.call(this);
"#;
    let call_apply = r#"
import { Foo } from "./base.js";
Foo.call.apply(Foo, [this]);
"#;
    for child in [renamed, param, namespace, call_apply] {
        let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
        assert_stays_function(code(&modules, "base.js"), "Foo");
    }
}

#[test]
fn namespace_iife_argument_pins_the_member() {
    let base = class_iife("Foo");
    let child = r#"
import * as ns from "./base.js";
(function (Base) { Base.call(this); })(ns.Foo);
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
}

#[test]
fn recognized_extends_call_apply_does_not_pin() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (e) {
    function t() {
        return e.call.apply(e, [this].concat([])) || this;
    }
    (function (ctor, sup) {
        ctor.prototype = Object.create(sup && sup.prototype, {
            constructor: { value: ctor, enumerable: false, writable: true, configurable: true }
        });
        if (sup) Object.setPrototypeOf(ctor, sup);
    })(t, e);
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    let child_out = code(&modules, "child.js");
    assert!(
        base_out.contains("class Foo"),
        "a recoverable extends call must not pin Foo:\nBASE:\n{base_out}\nCHILD:\n{}",
        code(&modules, "child.js")
    );
    assert!(
        child_out.contains("extends Foo"),
        "the child should become class extends:\n{child_out}"
    );
}

#[test]
fn pinned_subclass_cascades_to_its_superclass() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
export { Child };
"#;
    let grand = r#"
import { Child } from "./child.js";
var Grand = (function (Base) {
    function t() { return Base.call(this) || this; }
    t.prototype.leaf = function () { return 3; };
    return t;
})(Child);
export { Grand };
"#;
    let modules = unpack(
        &[("base.js", &base), ("child.js", child), ("grand.js", grand)],
        false,
    );
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert_stays_function(code(&modules, "child.js"), "Child");
    assert!(
        code(&modules, "grand.js").contains(".call"),
        "grandchild must keep the call:\n{}",
        code(&modules, "grand.js")
    );
}

#[test]
fn export_rename_and_default_iife_pin_the_returned_constructor() {
    let renamed = r#"
var t = (function () {
    function ctor() {}
    ctor.prototype.ping = function () { return 1; };
    return ctor;
})();
export { t as Foo };
"#;
    let default_iife = r#"
export default (function () {
    function t() {}
    t.prototype.ping = function () { return 1; };
    return t;
})();
"#;
    let named_child = r#"
import { Foo } from "./base.js";
Foo.call(this);
"#;
    let default_child = r#"
import Foo from "./base.js";
Foo.call(this);
"#;
    let named = unpack(&[("base.js", renamed), ("child.js", named_child)], false);
    assert_stays_function(code(&named, "base.js"), "Foo");
    let defaulted = unpack(
        &[("base.js", default_iife), ("child.js", default_child)],
        false,
    );
    let default_out = code(&defaulted, "base.js");
    assert!(
        !default_out
            .lines()
            .any(|line| line.trim().starts_with("class ")),
        "default IIFE must not contain a class:\n{default_out}"
    );
}

#[test]
fn function_declaration_with_prototype_stays_a_function() {
    let base = r#"
export function Foo() {}
Foo.prototype.ping = function () { return 1; };
"#;
    let child = r#"
import { Foo } from "./base.js";
Foo.call(this);
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        base_out.contains("function Foo"),
        "pinned function declaration must stay a function:\n{base_out}"
    );
    assert_stays_function(base_out, "Foo");
}

#[test]
fn two_inner_constructors_with_the_same_name_pin_only_the_export() {
    let base = r#"
var Foo = (function () {
    function t() {}
    t.prototype.ping = function () { return 1; };
    return t;
})();
var Bar = (function () {
    function t() {}
    t.prototype.ping = function () { return 2; };
    return t;
})();
export { Foo, Bar };
"#;
    let child = r#"
import { Foo } from "./base.js";
Foo.call(this);
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        !base_out.contains("class Foo") && base_out.contains("class Bar"),
        "only the called export stays a function:\n{base_out}"
    );
}

#[test]
fn pinned_provider_keeps_class_call_check() {
    let helper = r#"
function _classCallCheck(instance, constructor) {
    if (!(instance instanceof constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
var Foo = (function () {
    function t() { _classCallCheck(this, t); }
    t.prototype.ping = function () { return 1; };
    return t;
})();
export { Foo };
"#;
    let pinned = unpack(
        &[
            ("base.js", helper),
            (
                "child.js",
                "import { Foo } from \"./base.js\";\nFoo.call(this);\n",
            ),
        ],
        false,
    );
    let pinned_out = code(&pinned, "base.js");
    assert!(
        pinned_out.contains("_classCallCheck"),
        "pinned provider must keep the guard:\n{pinned_out}"
    );
    assert_stays_function(pinned_out, "Foo");

    let free = unpack(&[("base.js", helper)], false);
    let free_out = &free[0].1;
    assert!(
        free_out.contains("class Foo") && !free_out.contains("_classCallCheck"),
        "without a cross-file call the guard is consumed:\n{free_out}"
    );
}

#[test]
fn export_star_from_one_source_pins_the_definition() {
    let base = class_iife("Foo");
    let star = r#"export * from "./base.js";"#;
    let child = r#"
import { Foo } from "./star.js";
Foo.call(this);
"#;
    let modules = unpack(
        &[("base.js", &base), ("star.js", star), ("child.js", child)],
        false,
    );
    assert_stays_function(code(&modules, "base.js"), "Foo");
}

#[test]
fn conflicting_export_stars_do_not_pin() {
    let left = class_iife("Foo");
    let right = class_iife("Foo");
    let mid = r#"
export * from "./left.js";
export * from "./right.js";
"#;
    let child = r#"
import { Foo } from "./mid.js";
Foo.call(this);
"#;
    let modules = unpack(
        &[
            ("left.js", &left),
            ("right.js", &right),
            ("mid.js", mid),
            ("child.js", child),
        ],
        false,
    );
    assert!(
        code(&modules, "left.js").contains("class Foo")
            && code(&modules, "right.js").contains("class Foo"),
        "conflicting export * must not pin either definition"
    );
}

#[test]
fn assigned_extends_iife_still_pins_the_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child;
Child = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(
        code(&modules, "child.js").contains(".call"),
        "an assignment IIFE is not rewritten to super()"
    );
}

#[test]
fn unexported_middle_class_still_pins_the_imported_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Mid = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.mid = function () { return 1; };
    return t;
})(Foo);
var Child = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Mid);
export { Child };
"#;
    let grand = r#"
import { Child } from "./child.js";
Child.call(this);
"#;
    let modules = unpack(
        &[("base.js", &base), ("child.js", child), ("grand.js", grand)],
        false,
    );
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert_stays_function(code(&modules, "child.js"), "Child");
    assert!(code(&modules, "child.js").contains(".call"));
}

#[test]
fn flattened_wrapper_respects_the_pin() {
    let base = r#"
"use strict";
var e, t, n = (e = function e(t) {
    if (!(this instanceof e)) throw TypeError("Cannot call a class as a function");
    this.items = t;
}, t = [{ key: "get", value: function (e) { return this.items[e]; } }], function (e, t) {
    for (var n = 0; n < t.length; n++) {
        var i = t[n];
        i.enumerable = i.enumerable || false;
        i.configurable = true;
        if ("value" in i) i.writable = true;
        Object.defineProperty(e, i.key, i);
    }
}(e.prototype, t), e);
export { n as Foo };
"#;
    let child = r#"
import { Foo } from "./base.js";
function Sub(x) { Foo.call(this, x); }
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(code(&modules, "child.js").contains("Foo.call"));
}

#[test]
fn extends_iife_that_does_not_become_a_class_still_pins() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
function ccclass(name) { return function (ctor) { ctor.cc = name; return ctor; }; }
var Child = (function (_super) {
    __extends(Child, _super);
    function Child() { return _super !== null && _super.apply(this, arguments) || this; }
    Child.prototype.pong = function () { return 2; };
    Child = ccclass("Child")(Child);
    return Child;
}(Foo));
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(code(&modules, "child.js").contains(".apply"));
}

#[test]
fn nested_class_inside_default_iife_stays_a_function() {
    let base = r#"
export default (function () {
    var Foo = (function () {
        function t() {}
        t.prototype.ping = function () { return 1; };
        return t;
    })();
    return Foo;
})();
"#;
    let child = r#"
import Foo from "./base.js";
Foo.call(this);
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        !base_out
            .lines()
            .any(|line| line.trim().starts_with("class ")),
        "nested constructor must stay a function:\n{base_out}"
    );
}

#[test]
fn default_import_member_call_pins_the_export() {
    let base = class_iife("Foo");
    let child = r#"
import b from "./base.js";
function Sub() { b.Foo.call(this); }
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(code(&modules, "child.js").contains(".call"));
}

#[test]
fn exported_alias_of_a_constructor_stays_a_function() {
    let base = r#"
function e() {}
e.prototype.ping = function () { return 1; };
var Foo = e;
export { Foo };
"#;
    let child = r#"
import { Foo } from "./base.js";
Foo.call(this);
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(
        !code(&modules, "base.js").contains("class e"),
        "the aliased constructor must stay a function:\n{}",
        code(&modules, "base.js")
    );
}

#[test]
fn aliased_export_of_a_subclass_still_pins_the_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
var Other = Child;
export { Other };
"#;
    let grand = r#"
import { Other } from "./child.js";
function Sub() { Other.call(this); }
"#;
    let modules = unpack(
        &[("base.js", &base), ("child.js", child), ("grand.js", grand)],
        false,
    );
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(code(&modules, "child.js").contains(".call"));
}

#[test]
fn same_module_subclass_chain_does_not_pin_the_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Mid = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.mid = function () { return 1; };
    return t;
})(Foo);
var Child = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Mid);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "both subclasses become extends, so Foo stays a class:\n{}",
        code(&modules, "base.js")
    );
    let child_out = code(&modules, "child.js");
    assert!(child_out.contains("extends"));
    assert!(!child_out.contains(".call"), "{child_out}");
}

#[test]
fn babel_inherits_loose_import_does_not_pin_when_phase2_recovers_it() {
    let base = class_iife("Foo");
    let child = r#"
import _inheritsLoose from "@babel/runtime/helpers/inheritsLoose";
import { Foo } from "./base.js";
var Child = (function (_Foo) {
    _inheritsLoose(Child, _Foo);
    function Child() { return _Foo.call(this) || this; }
    Child.prototype.pong = function () { return 2; };
    return Child;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "Phase 2 consumes the call:\n{}",
        code(&modules, "base.js")
    );
    let child_out = code(&modules, "child.js");
    assert!(child_out.contains("extends"), "{child_out}");
    assert!(!child_out.contains(".call"), "{child_out}");
}

#[test]
fn source_map_emit_still_pins() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
Foo.call(this);
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], true);
    assert_stays_function(code(&modules, "base.js"), "Foo");
}

#[test]
fn method_call_keeps_the_other_constructor_a_function() {
    let base = class_iife("Base");
    let foo = class_iife("Foo");
    let child = r#"
import { Base } from "./base.js";
import { Foo } from "./foo.js";
var Other = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.o = function () { return 1; };
    return t;
})(Base);
var Mid = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.m = function () { return Other.call(this); };
    return t;
})(Foo);
export { Mid };
"#;
    let modules = unpack(
        &[("base.js", &base), ("foo.js", &foo), ("child.js", child)],
        false,
    );
    assert_stays_function(code(&modules, "base.js"), "Base");
    let child_out = code(&modules, "child.js");
    assert!(
        child_out.contains("Other.call"),
        "the method call must survive class recovery:\n{child_out}"
    );
}

#[test]
fn constructor_mixin_call_keeps_that_constructor_a_function() {
    let base = class_iife("Foo");
    let mixin = class_iife("Bar");
    let child = r#"
import { Foo } from "./base.js";
import { Bar } from "./mixin.js";
var Child = (function (_super) {
    __extends(t, _super);
    function t() {
        var self = _super.call(this) || this;
        Bar.call(self);
        return self;
    }
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(
        &[
            ("base.js", &base),
            ("mixin.js", &mixin),
            ("child.js", child),
        ],
        false,
    );
    assert_stays_function(code(&modules, "mixin.js"), "Bar");
    assert!(
        code(&modules, "child.js").contains("Bar.call"),
        "mixin call stays in the constructor:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn non_literal_concat_spread_still_pins_the_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (e) {
    __extends(t, e);
    function t(n) { return e.call.apply(e, [this].concat(n)) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(
        code(&modules, "child.js").contains(".call")
            || code(&modules, "child.js").contains(".apply"),
        "the spread call must remain:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn same_module_call_apply_still_pins_the_base() {
    let base = class_iife("Base");
    let child = r#"
import { Base } from "./base.js";
var Foo = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.ping = function () { return 1; };
    return t;
})(Base);
function make(x) { return Foo.call.apply(Foo, [x]); }
export { make };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Base");
    assert!(
        code(&modules, "child.js").contains(".call"),
        "Foo stays callable:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn minimal_level_does_not_consume_a_call_phase2_keeps() {
    let base = class_iife("Foo");
    let child = r#"
import { __extends } from "tslib";
import { Foo } from "./base.js";
var Child = (function (_super) {
    __extends(Child, _super);
    function Child() {
        return _super !== null && _super.apply(this, arguments) || this;
    }
    return Child;
})(Foo);
export { Child };
"#;
    let minimal = unpack_at(
        &[("base.js", &base), ("child.js", child)],
        false,
        RewriteLevel::Minimal,
    );
    assert_stays_function(code(&minimal, "base.js"), "Foo");
    assert!(
        code(&minimal, "child.js").contains(".apply"),
        "minimal keeps the apply:\n{}",
        code(&minimal, "child.js")
    );

    let standard = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&standard, "base.js").contains("class Foo"),
        "standard consumes the call:\n{}",
        code(&standard, "base.js")
    );
}

#[test]
fn minimal_pins_a_call_that_standard_rewrites_to_super() {
    let base = class_iife("Foo");
    let child = r#"
import { __extends } from "tslib";
import { Foo } from "./base.js";
var Child = (function (_super) {
    __extends(Child, _super);
    function Child() { return _super.call(this) || this; }
    Child.prototype.pong = function () { return 2; };
    return Child;
})(Foo);
export { Child };
"#;
    let files = [("base.js", base.as_str()), ("child.js", child)];
    let standard = unpack(&files, false);
    assert!(
        declares_class(code(&standard, "base.js"), "Foo"),
        "standard predicts super() and recovers the base:\n{}",
        code(&standard, "base.js")
    );
    let minimal = unpack_at(&files, false, RewriteLevel::Minimal);
    assert!(
        !declares_class(code(&minimal, "base.js"), "Foo"),
        "minimal pins every cross-file call:\n{}",
        code(&minimal, "base.js")
    );
}

#[test]
fn minimal_enumerable_accessor_does_not_consume_the_super_call() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (_super) {
    __extends(Child, _super);
    function Child() { return _super.call(this) || this; }
    Object.defineProperty(Child.prototype, "size", {
        enumerable: true,
        configurable: true,
        get: function () { return 1; }
    });
    return Child;
})(Foo);
export { Child };
"#;
    let minimal = unpack_at(
        &[("base.js", &base), ("child.js", child)],
        false,
        RewriteLevel::Minimal,
    );
    assert_stays_function(code(&minimal, "base.js"), "Foo");
    assert!(
        code(&minimal, "child.js").contains(".call"),
        "minimal keeps the call:\n{}",
        code(&minimal, "child.js")
    );
}

#[test]
fn direct_eval_keeps_the_unconverted_super_call() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Foo);
export { Child };
export function run(source) { return eval(source); }
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(
        code(&modules, "child.js").contains(".call"),
        "eval skips class recovery, so the call stays:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn nested_reused_var_keeps_the_super_call() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
export function make() {
    var Child;
    var Child = (function (_super) {
        __extends(t, _super);
        function t() { return _super.call(this) || this; }
        t.prototype.pong = function () { return 2; };
        return t;
    })(Foo);
    return Child;
}
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(
        code(&modules, "child.js").contains(".call"),
        "a repeated nested var is not rewritten to super():\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn returned_inner_class_iife_stays_a_function_when_the_export_is_called() {
    let base = r#"
var Outer = (function () {
    var Foo = (function () {
        function t() {}
        t.prototype.ping = function () { return 1; };
        return t;
    })();
    return Foo;
})();
export { Outer as Foo };
"#;
    let child = r#"
import { Foo } from "./base.js";
function Sub() { Foo.call(this); }
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        !base_out.contains("class "),
        "the returned constructor must stay a function:\n{base_out}"
    );
    assert!(
        code(&modules, "child.js").contains("Foo.call"),
        "the call stays:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn minimal_call_apply_is_not_consumed() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (e) {
    __extends(t, e);
    function t() { return e.call.apply(e, [this].concat([])) || this; }
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack_at(
        &[("base.js", &base), ("child.js", child)],
        false,
        RewriteLevel::Minimal,
    );
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(
        code(&modules, "child.js").contains(".apply")
            || code(&modules, "child.js").contains(".call"),
        "minimal leaves the apply:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn array_literal_apply_payload_does_not_pin() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (e) {
    __extends(t, e);
    function t(a) { return e.call.apply(e, [this, a]) || this; }
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "Phase 2 turns the apply payload into super():\n{}",
        code(&modules, "base.js")
    );
    assert!(
        code(&modules, "child.js").contains("extends"),
        "child becomes extends:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn rest_concat_apply_does_not_pin() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (e) {
    __extends(t, e);
    function t(...n) { return e.call.apply(e, [this].concat(n)) || this; }
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "a rest concat is consumed:\n{}",
        code(&modules, "base.js")
    );
}

#[test]
fn constructor_reused_var_keeps_the_super_call() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
export class Holder {
    constructor() {
        var Child;
        var Child = (function (_super) {
            __extends(t, _super);
            function t() { return _super.call(this) || this; }
            return t;
        })(Foo);
        this.made = new Child();
    }
}
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(
        code(&modules, "child.js").contains(".call"),
        "duplicate var in a constructor stays a call:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn labeled_var_keeps_the_super_call() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
export function make() {
    outer: var Child = (function (_super) {
        __extends(t, _super);
        function t() { return _super.call(this) || this; }
        return t;
    })(Foo);
    return Child;
}
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert_stays_function(code(&modules, "base.js"), "Foo");
    assert!(
        code(&modules, "child.js").contains(".call"),
        "a labeled var is not rewritten to super():\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn ident_alias_after_iife_return_stays_a_function() {
    let base = r#"
var Foo = (function () {
    function e() {}
    e.prototype.ping = function () { return 1; };
    var t = e;
    return t;
})();
export { Foo };
"#;
    let child = r#"
import { Foo } from "./base.js";
function Sub() { Foo.call(this); }
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        !base_out.contains("class "),
        "the aliased constructor must stay a function:\n{base_out}"
    );
    assert!(code(&modules, "child.js").contains("Foo.call"));
}

#[test]
fn default_object_member_call_pins_the_property() {
    let base = r#"
function Foo() { this.x = 1; }
Foo.prototype.ping = function () { return this.x; };
module.exports = { Foo: Foo };
"#;
    let child = r#"
var base = require("./base.js");
function Sub() { base.Foo.call(this); }
module.exports = Sub;
"#;
    let modules = unpack(&[("base.js", base), ("child.js", child)], false);
    let base_out = code(&modules, "base.js");
    assert!(
        !base_out.contains("class Foo") && !base_out.contains("class "),
        "Foo must stay a function:\n{base_out}"
    );
    assert!(
        code(&modules, "child.js").contains(".call"),
        "the member call stays:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn default_object_property_cascades_to_the_superclass() {
    let base = class_iife("Root");
    let mid = r#"
import { Root } from "./base.js";
var Sub = (function (_super) {
    __extends(t, _super);
    function t() { return _super.call(this) || this; }
    t.prototype.pong = function () { return 2; };
    return t;
})(Root);
export default { Pub: Sub };
"#;
    let leaf = r#"
import mid from "./mid.js";
function Use() { mid.Pub.call(this); }
"#;
    let modules = unpack(
        &[("base.js", &base), ("mid.js", mid), ("leaf.js", leaf)],
        false,
    );
    assert_stays_function(code(&modules, "base.js"), "Root");
    assert!(
        code(&modules, "mid.js").contains(".call"),
        "the subclass call stays:\n{}",
        code(&modules, "mid.js")
    );
}

#[test]
fn arguments_copy_loop_does_not_pin_the_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (_Foo) {
    __extends(Child, _Foo);
    function Child() {
        var _this;
        for (var _len = arguments.length, args = new Array(_len), _key = 0; _key < _len; _key++) {
            args[_key] = arguments[_key];
        }
        return _this = _Foo.call.apply(_Foo, [this].concat(args)) || this;
    }
    return Child;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "the arguments copy is consumed:\n{}",
        code(&modules, "base.js")
    );
    assert!(
        code(&modules, "child.js").contains("extends"),
        "child becomes extends:\n{}",
        code(&modules, "child.js")
    );
}

#[test]
fn declared_array_concat_does_not_pin_the_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
var Child = (function (e) {
    __extends(t, e);
    function t(a) {
        var extra = [a, 1];
        return e.call.apply(e, [this].concat(extra)) || this;
    }
    return t;
})(Foo);
export { Child };
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "a local array concat is consumed:\n{}",
        code(&modules, "base.js")
    );
}

#[test]
fn nested_local_extends_helper_does_not_pin_the_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
export function make() {
    function __extends(sub, sup) {
        sub.prototype = Object.create(sup.prototype);
    }
    var Child = (function (_super) {
        __extends(t, _super);
        function t() { return _super.call(this) || this; }
        t.prototype.pong = function () { return 2; };
        return t;
    })(Foo);
    return Child;
}
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "a nested helper still becomes extends, so Foo stays a class:\n{}",
        code(&modules, "base.js")
    );
    let child_out = code(&modules, "child.js");
    assert!(child_out.contains("extends"), "{child_out}");
    assert!(!child_out.contains(".call"), "{child_out}");
}

#[test]
fn switch_case_extends_does_not_pin_the_base() {
    let base = class_iife("Foo");
    let child = r#"
import { Foo } from "./base.js";
export function make(kind) {
    switch (kind) {
        case 0:
            var Child = (function (_super) {
                __extends(t, _super);
                function t() { return _super.call(this) || this; }
                return t;
            })(Foo);
            return Child;
    }
}
"#;
    let modules = unpack(&[("base.js", &base), ("child.js", child)], false);
    assert!(
        code(&modules, "base.js").contains("class Foo"),
        "a case-body extends consumes the call:\n{}",
        code(&modules, "base.js")
    );
}
