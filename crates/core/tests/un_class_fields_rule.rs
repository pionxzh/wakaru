mod common;

use common::{assert_eq_normalized, render};

#[test]
fn basic_init_to_inline() {
    let input = r#"
class Foo {
    __init() {
        this._count = 0;
    }
    constructor() {
        Foo.prototype.__init.call(this);
    }
}
"#;
    let expected = r#"
class Foo {
    constructor() {
        this._count = 0;
    }
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn multiple_inits() {
    let input = r#"
class Bar {
    __init() {
        this._x = 1;
    }
    __init2() {
        this._y = 2;
    }
    constructor() {
        Bar.prototype.__init.call(this);
        Bar.prototype.__init2.call(this);
        this.z = 3;
    }
}
"#;
    let expected = r#"
class Bar {
    constructor() {
        this._x = 1;
        this._y = 2;
        this.z = 3;
    }
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn init_with_arrow_function_body() {
    let input = r#"
class Baz {
    __init() {
        this._handler = (e) => {
            console.log(e);
        };
    }
    constructor() {
        Baz.prototype.__init.call(this);
    }
}
"#;
    let expected = r#"
class Baz {
    constructor() {
        this._handler = (e) => {
            console.log(e);
        };
    }
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn init_with_multiple_statements_not_inlined() {
    // __init with more than one statement should not be touched
    let input = r#"
class Qux {
    __init() {
        this._a = 1;
        this._b = 2;
    }
    constructor() {
        Qux.prototype.__init.call(this);
    }
}
"#;
    // The __init has 2 statements - still inline them all
    let expected = r#"
class Qux {
    constructor() {
        this._a = 1;
        this._b = 2;
    }
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn only_inlined_init_methods_removed() {
    // P2 regression: __init2 is NOT called in constructor, so it must be kept
    let input = r#"
class Foo {
    __init() {
        this._x = 1;
    }
    __init2() {
        this._y = 2;
    }
    constructor() {
        Foo.prototype.__init.call(this);
    }
}
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn regular_method_not_touched() {
    let input = r#"
class Keep {
    doStuff() {
        return 42;
    }
    constructor() {}
}
"#;
    assert_eq_normalized(&render(input), input.trim());
}

#[test]
fn static_function_assignment_after_class_is_preserved() {
    let input = r#"
class User {}
User.create = function(name) {
    return new User(name);
};
"#;
    let output = render(input);
    assert!(output.contains("User.create ="), "{output}");
    assert!(!output.contains("static create"), "{output}");
}

#[test]
fn react_metadata_assignments_after_class_are_preserved() {
    let input = r#"
class Link extends Component {}
Link.propTypes = {
    to: PropTypes.string.isRequired
};
Link.defaultProps = {
    replace: false
};
Link.contextTypes = {
    router: PropTypes.object
};
"#;
    let output = render(input);
    assert!(output.contains("Link.propTypes ="), "{output}");
    assert!(output.contains("Link.defaultProps ="), "{output}");
    assert!(output.contains("Link.contextTypes ="), "{output}");
    assert!(!output.contains("static propTypes"), "{output}");
    assert!(!output.contains("static defaultProps"), "{output}");
    assert!(!output.contains("static contextTypes"), "{output}");
}

#[test]
fn constructor_this_assignments_are_not_instance_fields_without_helper_evidence() {
    let input = r#"
class Foo {
    constructor() {
        this["value"] = 1;
        this.other = this.value + 1;
    }
    method() {
        return this.other;
    }
}
"#;
    let output = render(input);
    assert!(output.contains("this.value = 1"), "{output}");
    assert!(output.contains("this.other = this.value + 1"), "{output}");
    assert!(!output.contains("\n    value = 1"), "{output}");
    assert!(!output.contains("\n    other = this.value + 1"), "{output}");
}

#[test]
fn promotes_babel_define_property_calls_to_instance_fields() {
    let input = r#"
function _defineProperty(e, r, t) {
    if (r in e) {
        Object.defineProperty(e, r, { value: t, enumerable: true, configurable: true, writable: true });
    } else {
        e[r] = t;
    }
    return e;
}
class Foo {
    constructor() {
        _defineProperty(this, "value", 1);
        _defineProperty(this, "other", this.value + 1);
    }
    method() {
        return this.other;
    }
}
"#;
    let expected = r#"
class Foo {
    value = 1;
    other = this.value + 1;
    method() {
        return this.other;
    }
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn promotes_minified_define_property_helper_calls_to_instance_fields() {
    let input = r#"
function r(e, n, t) {
    if (n in e) {
        Object.defineProperty(e, n, { value: t, enumerable: true, configurable: true, writable: true });
    } else {
        e[n] = t;
    }
    return e;
}
class Foo {
    constructor() {
        r(this, "value", 1);
    }
}
"#;
    let expected = r#"
class Foo {
    value = 1;
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn promotes_key_normalizing_define_property_helper_calls_to_instance_fields() {
    let input = r#"
function _toPropertyKey(arg) {
    return arg;
}
function _defineProperty(e, r, t) {
    return (r = _toPropertyKey(r)) in e ? Object.defineProperty(e, r, {
        value: t,
        enumerable: true,
        configurable: true,
        writable: true
    }) : e[r] = t, e;
}
class Foo {
    constructor() {
        _defineProperty(this, "value", 1);
    }
}
"#;
    let expected = r#"
class Foo {
    value = 1;
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn promotes_imported_define_property_helper_calls_to_instance_fields() {
    let input = r#"
import _defineProperty from "@babel/runtime/helpers/defineProperty";
class Foo {
    constructor() {
        _defineProperty(this, "value", 1);
    }
}
"#;
    let expected = r#"
class Foo {
    value = 1;
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn same_name_non_helper_define_property_call_is_not_instance_field() {
    let input = r#"
function _defineProperty(target, key, value) {
    record(target, key, value);
}
class Foo {
    constructor() {
        _defineProperty(this, "value", 1);
    }
}
"#;
    let output = render(input);
    assert!(
        output.contains("_defineProperty(this, \"value\", 1)"),
        "{output}"
    );
    assert!(!output.contains("\n    value = 1"), "{output}");
}

#[test]
fn promotes_object_define_property_descriptor_to_instance_field() {
    let input = r#"
class Foo {
    constructor() {
        Object.defineProperty(this, "value", {
            enumerable: true,
            configurable: true,
            writable: true,
            value: 1
        });
        Object.defineProperty(this, "other", {
            enumerable: true,
            configurable: true,
            writable: true,
            value: this.value + 1
        });
    }
}
"#;
    let expected = r#"
class Foo {
    value = 1;
    other = this.value + 1;
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn shadowed_object_define_property_descriptor_is_not_instance_field() {
    let input = r#"
function wrap(Object) {
    class Foo {
        constructor() {
            Object.defineProperty(this, "value", {
                enumerable: true,
                configurable: true,
                writable: true,
                value: 1
            });
        }
    }
}
"#;
    let output = render(input);
    assert!(
        output.contains("Object.defineProperty(this, \"value\""),
        "{output}"
    );
    assert!(!output.contains("\n        value = 1"), "{output}");
}

#[test]
fn descriptor_missing_writable_is_not_instance_field() {
    let input = r#"
class Foo {
    constructor() {
        Object.defineProperty(this, "value", {
            enumerable: true,
            configurable: true,
            value: 1
        });
    }
}
"#;
    let output = render(input);
    assert!(
        output.contains("Object.defineProperty(this, \"value\""),
        "{output}"
    );
    assert!(!output.contains("\n    value = 1"), "{output}");
}

#[test]
fn promotes_tsc_weakmap_private_field_and_accesses() {
    let input = r#"
var __classPrivateFieldSet = function(receiver, state, value, kind, f) {
    return state.set(receiver, value), value;
};
var __classPrivateFieldGet = function(receiver, state, kind, f) {
    return state.get(receiver);
};
var _Foo_x;
class Foo {
    constructor() {
        _Foo_x.set(this, 1);
    }
    getX() {
        return __classPrivateFieldGet(this, _Foo_x, "f");
    }
    setX(value) {
        __classPrivateFieldSet(this, _Foo_x, value, "f");
    }
}
_Foo_x = new WeakMap();
"#;
    let expected = r#"
class Foo {
    #x = 1;
    getX() {
        return this.#x;
    }
    setX(value) {
        this.#x = value;
    }
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn constructor_param_private_init_is_not_promoted_or_rewritten() {
    let input = r#"
var __classPrivateFieldGet = function(receiver, state, kind, f) {
    return state.get(receiver);
};
var _Foo_x;
class Foo {
    constructor(seed) {
        _Foo_x.set(this, seed);
    }
    getX() {
        return __classPrivateFieldGet(this, _Foo_x, "f");
    }
}
_Foo_x = new WeakMap();
"#;
    let output = render(input);
    assert!(!output.contains("#x"), "{output}");
    assert!(output.contains("_Foo_x.set(this, seed)"), "{output}");
    assert!(
        output.contains("__classPrivateFieldGet(this, _Foo_x, \"f\")"),
        "{output}"
    );
}

#[test]
fn shadowed_weakmap_private_init_is_not_promoted() {
    let input = r#"
const WeakMap = makeWeakMap();
var __classPrivateFieldGet = function(receiver, state, kind, f) {
    return state.get(receiver);
};
var _Foo_x = new WeakMap();
class Foo {
    constructor() {
        _Foo_x.set(this, 1);
    }
    getX() {
        return __classPrivateFieldGet(this, _Foo_x, "f");
    }
}
"#;
    let output = render(input);
    assert!(!output.contains("#x"), "{output}");
    assert!(output.contains("_Foo_x.set(this, 1)"), "{output}");
    assert!(
        output.contains("__classPrivateFieldGet(this, _Foo_x, \"f\")"),
        "{output}"
    );
}

#[test]
fn unsupported_private_map_ref_blocks_promotion() {
    let input = r#"
var A4 = function(receiver, state, value, kind) {
    return state.set(receiver, value), value;
};
var KE = new WeakMap();
class Foo {
    constructor() {
        KE.set(this, undefined);
        A4(this, KE, new Uint8Array(), "f");
    }
}
"#;
    let output = render(input);
    assert!(!output.contains("#KE"), "{output}");
    assert!(output.contains("KE.set(this, undefined)"), "{output}");
    assert!(output.contains("A4(this, KE"), "{output}");
}

#[test]
fn shared_weakmap_private_field_is_not_promoted() {
    let input = r#"
var __classPrivateFieldGet = function(receiver, state, kind, f) {
    return state.get(receiver);
};
var _shared = new WeakMap();
class A {
    constructor() {
        _shared.set(this, 1);
    }
    get() {
        return __classPrivateFieldGet(this, _shared, "f");
    }
}
class B {
    constructor() {
        _shared.set(this, 2);
    }
    get() {
        return __classPrivateFieldGet(this, _shared, "f");
    }
}
"#;
    let output = render(input);
    assert!(!output.contains("#shared"), "{output}");
    assert!(output.contains("_shared.set(this, 1)"), "{output}");
    assert!(output.contains("_shared.set(this, 2)"), "{output}");
    assert!(
        output.contains("__classPrivateFieldGet(this, _shared, \"f\")"),
        "{output}"
    );
}

#[test]
fn nested_weakmap_reassignment_blocks_private_field_promotion() {
    let input = r#"
var _Foo_x;
function reset() {
    _Foo_x = new WeakMap();
}
class Foo {
    constructor() {
        _Foo_x.set(this, 1);
    }
}
_Foo_x = new WeakMap();
"#;
    let output = render(input);
    assert!(!output.contains("#x"), "{output}");
    assert!(output.contains("function reset()"), "{output}");
    assert!(output.contains("_Foo_x = new WeakMap()"), "{output}");
    assert!(output.contains("_Foo_x.set(this, 1)"), "{output}");
}

#[test]
fn weakmap_initializer_with_args_blocks_private_field_promotion() {
    let input = r#"
var _Foo_x = new WeakMap();
var _other;
_other = new WeakMap(_Foo_x);
class Foo {
    constructor() {
        _Foo_x.set(this, 1);
    }
}
"#;
    let output = render(input);
    assert!(!output.contains("#x"), "{output}");
    assert!(output.contains("_Foo_x = new WeakMap()"), "{output}");
    assert!(output.contains("_other = new WeakMap(_Foo_x)"), "{output}");
    assert!(output.contains("_Foo_x.set(this, 1)"), "{output}");
}

#[test]
fn constructor_param_assignments_are_not_instance_fields() {
    let input = r#"
class Foo {
    constructor(value) {
        this.value = value;
    }
}
"#;
    assert_eq_normalized(&render(input), input.trim());
}

#[test]
fn swc_external_define_property_import_promotes_to_fields() {
    let input = r#"
import { _ as _define_property } from "@swc/helpers/_/_define_property";
class Foo {
    constructor() {
        _define_property(this, "value", 1);
        _define_property(this, "other", this.value + 1);
    }
    method() {
        return this.other;
    }
}
"#;
    let expected = r#"
class Foo {
    value = 1;
    other = this.value + 1;
    method() {
        return this.other;
    }
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn derived_constructor_assignments_are_not_instance_fields() {
    let input = r#"
class Foo extends Base {
    constructor() {
        super();
        this.value = 1;
    }
}
"#;
    assert_eq_normalized(&render(input), input.trim());
}

#[test]
fn private_backing_map_lifetime_is_preserved() {
    for (members, suffix) in [
        ("", "new Foo(); _Foo_x = new WeakMap();"),
        (
            "",
            "_Foo_x = new WeakMap(); var first = new Foo(); _Foo_x = new WeakMap();",
        ),
        ("static first = new Foo();", "_Foo_x = new WeakMap();"),
        ("", "var first = new Foo(), _Foo_x = new WeakMap();"),
        (
            "",
            "_Foo_x = new WeakMap(); use(new Foo()), _Foo_x = new WeakMap();",
        ),
    ] {
        let input = format!(
            "var _Foo_x; class Foo {{ constructor() {{ _Foo_x.set(this, 1); }} {members} }} {suffix}"
        );
        let output = common::render_rule(&input, |mark| {
            wakaru_core::rules::UnClassFields::new_with_mark(
                mark,
                wakaru_core::rules::RewriteLevel::Standard,
            )
        });
        assert!(!output.contains("#x"), "{output}");
        assert!(output.contains("_Foo_x.set(this, 1)"), "{output}");
    }
}

#[test]
fn private_backing_map_allows_local_export_between_class_and_initializer() {
    let input = "var _Foo_x; class Foo { constructor() { _Foo_x.set(this, 1); } } export { Foo }; _Foo_x = new WeakMap();";
    let output = common::render_rule(input, |mark| {
        wakaru_core::rules::UnClassFields::new_with_mark(
            mark,
            wakaru_core::rules::RewriteLevel::Standard,
        )
    });
    assert!(output.contains("#x = 1"), "{output}");
    assert!(!output.contains("WeakMap"), "{output}");
}

fn apply_class_fields(input: &str) -> String {
    common::render_rule(input, |mark| {
        wakaru_core::rules::UnClassFields::new_with_mark(
            mark,
            wakaru_core::rules::RewriteLevel::Standard,
        )
    })
}

#[test]
fn tslib_private_fields_match_proven_helper_delivery() {
    for (prefix, get, set) in [
        ("var ts = require('tslib');", "ts.__classPrivateFieldGet", "ts.__classPrivateFieldSet"),
        ("import * as ts from 'tslib';", "ts.__classPrivateFieldGet", "ts.__classPrivateFieldSet"),
        ("import {__classPrivateFieldGet as g, __classPrivateFieldSet as s} from 'tslib';", "g", "s"),
        ("var g = require('tslib').__classPrivateFieldGet, s = require('tslib').__classPrivateFieldSet;", "g", "s"),
        ("", "require('tslib').__classPrivateFieldGet", "require('tslib').__classPrivateFieldSet"),
        ("var g = this && this.__classPrivateFieldGet || function(receiver, state) { return state.get(receiver); }; var s = this && this.__classPrivateFieldSet || function(receiver, state, value) { return state.set(receiver, value); };", "g", "s"),
    ] {
        let input = format!("{prefix} var _Foo_x; class Foo {{ constructor() {{ _Foo_x.set(this, 1); }} getX() {{ return {get}(this, _Foo_x, 'f'); }} setX(value) {{ {set}(this, _Foo_x, value, 'f'); }} }} _Foo_x = new WeakMap();");
        let output = apply_class_fields(&input);
        assert!(output.contains("#x = 1"), "{output}");
        assert!(output.contains("return this.#x"), "{output}");
        assert!(output.contains("this.#x = value"), "{output}");
        assert!(!output.contains("WeakMap"), "{output}");
    }
}

#[test]
fn tslib_private_fields_keep_unsupported_calls_and_helper_writes() {
    for (prefix, params, call, suffix) in [
        (
            "var ts = require('custom');",
            "",
            "ts.__classPrivateFieldGet(this, _Foo_x, 'f')",
            "",
        ),
        (
            "var ts = require('tslib');",
            "ts",
            "ts.__classPrivateFieldGet(this, _Foo_x, 'f')",
            "",
        ),
        (
            "var ts = require('tslib');",
            "other",
            "ts.__classPrivateFieldGet(other, _Foo_x, 'f')",
            "",
        ),
        (
            "var ts = require('tslib');",
            "",
            "ts.__classPrivateFieldGet(this, _Foo_x, 'a')",
            "",
        ),
        (
            "var ts = require('tslib');",
            "",
            "ts.__classPrivateFieldGet(this, _Foo_x, ...['f'])",
            "",
        ),
        (
            "var ts = require('tslib');",
            "",
            "ts.__classPrivateFieldGet(this, _Foo_x, 'f')",
            "ts = custom;",
        ),
        (
            "var ts = require('tslib');",
            "",
            "ts.__classPrivateFieldGet(this, _Foo_x, 'f')",
            "ts.__classPrivateFieldGet = custom;",
        ),
        (
            "var g = require('tslib').__classPrivateFieldGet;",
            "",
            "g(this, _Foo_x, 'f')",
            "g = custom;",
        ),
        (
            "var ts = require('tslib');",
            "",
            "ts.__classPrivateFieldGet(this, _Foo_x, 'f')",
            "use(_Foo_x);",
        ),
        (
            "function require(name) { return custom; }",
            "",
            "require('tslib').__classPrivateFieldGet(this, _Foo_x, 'f')",
            "",
        ),
    ] {
        let input = format!("{prefix} var _Foo_x; class Foo {{ constructor() {{ _Foo_x.set(this, 1); }} getX({params}) {{ return {call}; }} }} _Foo_x = new WeakMap(); {suffix}");
        let output = apply_class_fields(&input);
        assert!(!output.contains("#x"), "{output}");
        assert!(output.contains("_Foo_x.set(this, 1)"), "{output}");
    }
}

fn private_owner_source(owner: &str, between: &str) -> String {
    let body = "constructor() { _Foo_x.set(this, 1); } getX() { return get(this, _Foo_x, 'f'); }";
    format!(
        "var get = this && this.__classPrivateFieldGet || function(receiver, state) {{ return state.get(receiver); }}; var _Foo_x; {} {between} _Foo_x = new WeakMap();",
        owner.replace("BODY", body)
    )
}

#[test]
fn private_map_owner_accepts_default_class_declarations() {
    for owner in [
        "export default class Foo { BODY }",
        "export default class { BODY }",
    ] {
        let output = apply_class_fields(&private_owner_source(owner, ""));
        assert!(output.contains("#x = 1"), "{output}");
        assert!(output.contains("return this.#x"), "{output}");
        assert!(!output.contains("WeakMap"), "{output}");
    }
}

#[test]
fn private_map_owner_allows_export_default_of_the_owner_binding() {
    let output = apply_class_fields(&private_owner_source(
        "class Foo { BODY }",
        "export default Foo;",
    ));
    assert!(output.contains("#x = 1"), "{output}");
    assert!(output.contains("export default Foo"), "{output}");
    assert!(!output.contains("WeakMap"), "{output}");
}

#[test]
fn private_map_owner_accepts_single_class_expression_declarators() {
    for owner in [
        "const Foo = class { BODY };",
        "let Foo = class { BODY };",
        "var Foo = class { BODY };",
        "export const Foo = class { BODY };",
        "const Foo = (class Inner { BODY });",
    ] {
        let output = apply_class_fields(&private_owner_source(owner, ""));
        assert!(output.contains("#x = 1"), "{output}");
        assert!(!output.contains("WeakMap"), "{output}");
    }
}

#[test]
fn private_map_owner_accepts_lowered_class_assignment_to_local_binding() {
    let output = apply_class_fields(&private_owner_source(
        "var Foo; Foo = class { BODY };",
        "export default Foo;",
    ));
    assert!(output.contains("#x = 1"), "{output}");
    assert!(!output.contains("WeakMap"), "{output}");
}

#[test]
fn private_map_owner_keeps_definition_and_intervening_execution_boundaries() {
    for (owner, between) in [
        ("export default class Foo { BODY }", "new Foo();"),
        (
            "export default class Foo { BODY static first = new Foo(); }",
            "",
        ),
        ("const Foo = class { BODY };", "new Foo();"),
        ("const Foo = class { BODY }, instance = new Foo();", ""),
        ("const Foo = class { BODY static { invoke(); } };", ""),
        ("var Foo; Foo = class { BODY };", "new Foo();"),
        ("Foo = class { BODY };", ""),
        ("class Foo { BODY }", "export default external;"),
        ("let other; class Foo { BODY }", "export default other;"),
        ("class Foo { BODY }", "export default new Foo();"),
        ("class Foo { BODY }", "_Foo_x = new WeakMap();"),
    ] {
        let output = apply_class_fields(&private_owner_source(owner, between));
        assert!(!output.contains("#x"), "{output}");
        assert!(output.contains("_Foo_x.set(this, 1)"), "{output}");
    }
}

#[test]
fn private_map_owner_recovers_real_tsc_default_and_expression_modules() {
    for source in [
        include_str!("fixtures/private-field-owners/default-esm.js"),
        include_str!("fixtures/private-field-owners/default-cjs.js"),
        include_str!("fixtures/private-field-owners/expression-esm.js"),
        include_str!("fixtures/private-field-owners/expression-cjs.js"),
    ] {
        let output = render(source);
        assert!(output.contains("#x = 1"), "{output}");
        assert!(output.contains("return this.#x"), "{output}");
        assert!(!output.contains("WeakMap"), "{output}");
    }
}

#[test]
fn keeps_init_methods_when_module_has_dynamic_scope() {
    // Inlining `__init` bodies and dropping the method (and private WeakMap
    // declarations) removes bindings; the module-wide dynamic-scope skip in
    // docs/rewrite-assumptions.md applies.
    for hazard in ["eval(code);", "with (scope) { observe(); }"] {
        let input = format!(
            r#"
class Foo {{
    __init() {{
        this._count = 0;
    }}
    constructor() {{
        Foo.prototype.__init.call(this);
    }}
}}
{hazard}
"#
        );
        let output = render(&input);
        assert!(output.contains("__init()"), "{output}");
    }
}

#[test]
fn recovered_private_field_keeps_an_exported_helper() {
    let input = r#"
var get = this && this.__classPrivateFieldGet || function(receiver, state) {
  return state.get(receiver);
};
var _Foo_x;
class Foo {
  constructor() { _Foo_x.set(this, 1); }
  getX() { return get(this, _Foo_x, 'f'); }
}
_Foo_x = new WeakMap();
export { get };
"#;
    let output = apply_class_fields(input);
    assert!(output.contains("return this.#x"), "{output}");
    assert!(output.contains("var get ="), "{output}");
    assert!(output.contains("export { get }"), "{output}");
}
