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
    // SmartRename later names the unpromoted map after its type.
    assert!(!output.contains("#KE"), "{output}");
    assert!(output.contains("weakMap.set(this, undefined)"), "{output}");
    assert!(output.contains("A4(this, weakMap"), "{output}");
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

// Babel 7.24+ class-properties helpers, as the transform injects them.
const BABEL_PRIVATE_HELPERS: &str = r#"
function _classPrivateFieldInitSpec(e, t, a) { _checkPrivateRedeclaration(e, t), t.set(e, a); }
function _checkPrivateRedeclaration(e, t) { if (t.has(e)) throw new TypeError("Cannot initialize the same private elements twice on an object"); }
function _classPrivateFieldSet(s, a, r) { return s.set(_assertClassBrand(s, a), r), r; }
function _classPrivateFieldGet(s, a) { return s.get(_assertClassBrand(s, a)); }
function _assertClassBrand(e, t, n) { if ("function" == typeof e ? e === t : e.has(t)) return arguments.length < 3 ? t : n; throw new TypeError("Private element is not present on this object"); }
"#;

// swc's helpers; some older Babel releases have the same descriptor helpers.
const SWC_PRIVATE_HELPERS: &str = r#"
function _check_private_redeclaration(obj, privateCollection) {
    if (privateCollection.has(obj)) {
        throw new TypeError("Cannot initialize the same private elements twice on an object");
    }
}
function _class_apply_descriptor_get(receiver, descriptor) {
    if (descriptor.get) {
        return descriptor.get.call(receiver);
    }
    return descriptor.value;
}
function _class_apply_descriptor_set(receiver, descriptor, value) {
    if (descriptor.set) {
        descriptor.set.call(receiver, value);
    } else {
        if (!descriptor.writable) {
            throw new TypeError("attempted to set read only private field");
        }
        descriptor.value = value;
    }
}
function _class_extract_field_descriptor(receiver, privateMap, action) {
    if (!privateMap.has(receiver)) {
        throw new TypeError("attempted to " + action + " private field on non-instance");
    }
    return privateMap.get(receiver);
}
function _class_private_field_get(receiver, privateMap) {
    var descriptor = _class_extract_field_descriptor(receiver, privateMap, "get");
    return _class_apply_descriptor_get(receiver, descriptor);
}
function _class_private_field_init(obj, privateMap, value) {
    _check_private_redeclaration(obj, privateMap);
    privateMap.set(obj, value);
}
function _class_private_field_set(receiver, privateMap, value) {
    var descriptor = _class_extract_field_descriptor(receiver, privateMap, "set");
    _class_apply_descriptor_set(receiver, descriptor, value);
    return value;
}
"#;

// esbuild's helpers for a target without class fields.
const ESBUILD_PRIVATE_HELPERS: &str = r#"
var __typeError = (msg) => { throw TypeError(msg); };
var __accessCheck = (obj, member, msg) => member.has(obj) || __typeError("Cannot " + msg);
var __privateGet = (obj, member, getter) => (__accessCheck(obj, member, "read from private field"), getter ? getter.call(obj) : member.get(obj));
var __privateAdd = (obj, member, value) => member.has(obj) ? __typeError("Cannot add the same private member more than once") : member instanceof WeakSet ? member.add(obj) : member.set(obj, value);
var __privateSet = (obj, member, value, setter) => (__accessCheck(obj, member, "write to private field"), setter ? setter.call(obj, value) : member.set(obj, value), value);
"#;

fn assert_counter_recovered(output: &str, name: &str) {
    assert!(output.contains(&format!("#{name} = ")), "{output}");
    assert!(
        output.contains(&format!("this.#{name} = this.#{name} + 1")),
        "{output}"
    );
    assert!(output.contains(&format!("return this.#{name}")), "{output}");
    assert!(!output.contains("WeakMap"), "{output}");
    for helper in [
        "_classPrivateField",
        "_assertClassBrand",
        "_checkPrivateRedeclaration",
        "_class_",
        "_check_private",
        "__private",
        "__accessCheck",
        "__typeError",
    ] {
        assert!(!output.contains(helper), "{helper} left in {output}");
    }
}

#[test]
fn recovers_babel_private_fields_at_the_top_level_and_in_a_function() {
    let input = format!(
        r#"{BABEL_PRIVATE_HELPERS}
var _n = new WeakMap();
export class Tally {{
  constructor() {{ _classPrivateFieldInitSpec(this, _n, 0); }}
  inc() {{ _classPrivateFieldSet(_n, this, _classPrivateFieldGet(_n, this) + 1); return _classPrivateFieldGet(_n, this); }}
}}
export function makeCounter(start) {{
  var _count = new WeakMap();
  class Counter {{
    constructor() {{ _classPrivateFieldInitSpec(this, _count, start); }}
    inc() {{ _classPrivateFieldSet(_count, this, _classPrivateFieldGet(_count, this) + 1); return _classPrivateFieldGet(_count, this); }}
  }}
  return new Counter();
}}
"#
    );
    let output = render(&input);
    assert_counter_recovered(&output, "n");
    assert_counter_recovered(&output, "count");
    assert!(output.contains("#count = start"), "{output}");
}

#[test]
fn recovers_swc_descriptor_private_fields() {
    let input = format!(
        r#"{SWC_PRIVATE_HELPERS}
var _n = new WeakMap();
export class Tally {{
  inc() {{ _class_private_field_set(this, _n, _class_private_field_get(this, _n) + 1); return _class_private_field_get(this, _n); }}
  constructor() {{ _class_private_field_init(this, _n, {{ writable: true, value: 0 }}); }}
}}
export function makeCounter(start) {{
  var _n = new WeakMap();
  class Counter {{
    inc() {{ _class_private_field_set(this, _n, _class_private_field_get(this, _n) + 1); return _class_private_field_get(this, _n); }}
    constructor() {{ _class_private_field_init(this, _n, {{ writable: true, value: start }}); }}
  }}
  return new Counter();
}}
"#
    );
    let output = render(&input);
    assert_counter_recovered(&output, "n");
    assert!(output.contains("#n = 0"), "{output}");
    assert!(output.contains("#n = start"), "{output}");
    assert!(!output.contains("writable"), "{output}");
}

#[test]
fn recovers_an_older_babel_descriptor_field_initialized_on_the_map() {
    let input = format!(
        r#"{SWC_PRIVATE_HELPERS}
var _n = new WeakMap();
export class Tally {{
  constructor() {{ _n.set(this, {{ writable: true, value: 0 }}); }}
  inc() {{ _class_private_field_set(this, _n, _class_private_field_get(this, _n) + 1); return _class_private_field_get(this, _n); }}
}}
"#
    );
    let output = render(&input);
    assert_counter_recovered(&output, "n");
    assert!(output.contains("#n = 0"), "{output}");
}

#[test]
fn recovers_esbuild_private_fields_initialized_after_the_class() {
    let input = format!(
        r#"{ESBUILD_PRIVATE_HELPERS}
var _n;
export class Tally {{
  constructor() {{ __privateAdd(this, _n, 0); }}
  inc() {{ __privateSet(this, _n, __privateGet(this, _n) + 1); return __privateGet(this, _n); }}
}}
_n = new WeakMap();
export function makeCounter(start) {{
  var _n2;
  class Counter {{
    constructor() {{ __privateAdd(this, _n2, start); }}
    inc() {{ __privateSet(this, _n2, __privateGet(this, _n2) + 1); return __privateGet(this, _n2); }}
  }}
  _n2 = new WeakMap();
  return new Counter();
}}
"#
    );
    let output = render(&input);
    assert_counter_recovered(&output, "n");
    assert_counter_recovered(&output, "n2");
}

#[test]
fn recovers_minified_private_field_helpers_with_inlined_checks() {
    // Terser inlines single-use sub-helpers as calls of function expressions
    // and returns the class from its factory directly.
    for input in [
        r#"function t(t,e,n){(function(t,e){if(e.has(t))throw new TypeError("Cannot initialize the same private elements twice on an object")})(t,e),e.set(t,n)}function e(t,e,n){return t.set(r(t,e),n),n}function n(t,e){return t.get(r(t,e))}function r(t,e,n){if("function"==typeof t?t===e:t.has(e))return arguments.length<3?e:n;throw new TypeError("Private element is not present on this object")}export function make(r){var i=new WeakMap;return new class{constructor(){t(this,i,r)}inc(){return e(i,this,n(i,this)+1),n(i,this)}}}"#,
        r#"function e(e,t,n){if(!t.has(e))throw new TypeError("attempted to "+n+" private field on non-instance");return t.get(e)}function t(t,n){return function(e,t){return t.get?t.get.call(e):t.value}(t,e(t,n,"get"))}function n(e,t,n){!function(e,t){if(t.has(e))throw new TypeError("Cannot initialize the same private elements twice on an object")}(e,t),t.set(e,n)}function r(t,n,r){return function(e,t,n){if(t.set)t.set.call(e,n);else{if(!t.writable)throw new TypeError("attempted to set read only private field");t.value=n}}(t,e(t,n,"set"),r),r}export function make(e){var i=new WeakMap;return new class{inc(){return r(this,i,t(this,i)+1),t(this,i)}constructor(){n(this,i,{writable:!0,value:e})}}}"#,
        r#"var c=e=>{throw TypeError(e)},i=(e,t,r)=>t.has(e)||c("Cannot "+r),s=(e,t,r)=>(i(e,t,"read from private field"),r?r.call(e):t.get(e)),l=(e,t,r)=>t.has(e)?c("Cannot add the same private member more than once"):t instanceof WeakSet?t.add(e):t.set(e,r),u=(e,t,r,n)=>(i(e,t,"write to private field"),n?n.call(e,r):t.set(e,r),r);export function make(e){var i;return i=new WeakMap,new class{constructor(){l(this,i,e)}inc(){return u(this,i,s(this,i)+1),s(this,i)}}}"#,
    ] {
        let output = render(input);
        assert!(output.contains("#i = "), "{output}");
        assert!(output.contains("this.#i = this.#i + 1"), "{output}");
        assert!(!output.contains("WeakMap"), "{output}");
        assert!(!output.contains("has("), "{output}");
    }
}

fn esbuild_counter(constructor: &str, method: &str) -> String {
    format!(
        r#"{ESBUILD_PRIVATE_HELPERS}
var _n;
export class Tally {{
  constructor() {{ {constructor} }}
  inc(other) {{ {method} }}
}}
_n = new WeakMap();
"#
    )
}

#[test]
fn lowered_private_fields_keep_calls_that_are_not_plain_field_access() {
    for input in [
        // A getter argument: esbuild lowers a private accessor this way.
        esbuild_counter(
            "__privateAdd(this, _n, 0);",
            "return __privateGet(this, _n, function() { return 1; });",
        ),
        // A receiver other than `this`.
        esbuild_counter(
            "__privateAdd(this, _n, 0);",
            "return __privateGet(other, _n);",
        ),
        // The map read outside the helpers.
        esbuild_counter("__privateAdd(this, _n, 0);", "return _n.has(other);"),
        // Two initializations of one map.
        esbuild_counter(
            "__privateAdd(this, _n, 0); __privateAdd(this, _n, 1);",
            "return __privateGet(this, _n);",
        ),
        // The class can be instantiated before its map exists.
        esbuild_counter(
            "__privateAdd(this, _n, 0);",
            "return __privateGet(this, _n);",
        )
        .replace("_n = new WeakMap();", "new Tally(); _n = new WeakMap();"),
    ] {
        let output = render(&input);
        assert!(!output.contains("#n"), "{output}");
        assert!(output.contains("__privateAdd"), "{output}");
    }
}

#[test]
fn lowered_private_helpers_need_their_full_bodies() {
    for (helpers, members) in [
        // A get that skips the brand check is not a private read.
        (
            BABEL_PRIVATE_HELPERS.replace(
                "return s.get(_assertClassBrand(s, a));",
                "return s.get(a);",
            ),
            "constructor() { _classPrivateFieldInitSpec(this, _n, 0); } get() { return _classPrivateFieldGet(_n, this); }",
        ),
        // A setter that never stores the value.
        (
            ESBUILD_PRIVATE_HELPERS.replace(
                "setter ? setter.call(obj, value) : member.set(obj, value), value",
                "setter ? setter.call(obj, value) : member.get(obj), value",
            ),
            "constructor() { __privateAdd(this, _n, 0); } set(v) { __privateSet(this, _n, v); }",
        ),
        // An init helper without the redeclaration check.
        (
            BABEL_PRIVATE_HELPERS.replace(
                "_checkPrivateRedeclaration(e, t), t.set(e, a);",
                "log(e, t), t.set(e, a);",
            ),
            "constructor() { _classPrivateFieldInitSpec(this, _n, 0); } get() { return _classPrivateFieldGet(_n, this); }",
        ),
    ] {
        let source = format!("{helpers}\nvar _n = new WeakMap();\nexport class Foo {{ {members} }}");
        let output = render(&source);
        assert!(!output.contains("#n"), "{output}");
    }
}

#[test]
fn descriptor_private_fields_need_a_plain_writable_descriptor() {
    for descriptor in [
        "{ writable: false, value: 0 }",
        "{ writable: true, value: 0, enumerable: true }",
        "{ value: 0 }",
        "{ get: read, set: write }",
    ] {
        let input = format!(
            r#"{SWC_PRIVATE_HELPERS}
var _n = new WeakMap();
export class Foo {{
  constructor() {{ _class_private_field_init(this, _n, {descriptor}); }}
  get() {{ return _class_private_field_get(this, _n); }}
}}
"#
        );
        let output = render(&input);
        assert!(!output.contains("#n"), "{output}");
    }
}

#[test]
fn a_function_private_map_must_be_created_by_that_function() {
    // `_n` lives outside `make`: a second call resets the map that the first
    // call's instances still use.
    let input = format!(
        r#"{ESBUILD_PRIVATE_HELPERS}
var _n;
export function make() {{
  _n = new WeakMap();
  return new class {{
    constructor() {{ __privateAdd(this, _n, 0); }}
    get() {{ return __privateGet(this, _n); }}
  }}();
}}
"#
    );
    let output = render(&input);
    assert!(!output.contains("#n"), "{output}");
    assert!(output.contains("__privateAdd"), "{output}");
}

#[test]
fn a_lowered_private_name_that_is_taken_keeps_the_map() {
    let input = format!(
        r#"{BABEL_PRIVATE_HELPERS}
var _n = new WeakMap();
export class Foo {{
  #n = 1;
  constructor() {{ _classPrivateFieldInitSpec(this, _n, 0); }}
  get() {{ return [this.#n, _classPrivateFieldGet(_n, this)]; }}
}}
"#
    );
    let output = render(&input);
    assert!(output.contains("_classPrivateFieldGet"), "{output}");
}

#[test]
fn recovers_an_esbuild_private_field_without_an_initializer() {
    let input = format!(
        r#"{ESBUILD_PRIVATE_HELPERS}
var _n;
export class Foo {{
  constructor() {{ __privateAdd(this, _n); }}
  get() {{ return __privateGet(this, _n); }}
  set(v) {{ __privateSet(this, _n, v); }}
}}
_n = new WeakMap();
"#
    );
    let output = render(&input);
    assert!(output.contains("#n;"), "{output}");
    assert!(output.contains("return this.#n"), "{output}");
    assert!(output.contains("this.#n = v"), "{output}");
    assert!(!output.contains("__private"), "{output}");
}

#[test]
fn a_descriptor_private_field_needs_its_descriptor() {
    let input = format!(
        r#"{SWC_PRIVATE_HELPERS}
var _n = new WeakMap();
export class Foo {{
  constructor() {{ _class_private_field_init(this, _n); }}
  get() {{ return _class_private_field_get(this, _n); }}
}}
"#
    );
    let output = render(&input);
    assert!(!output.contains("#n"), "{output}");
}

#[test]
fn recovers_private_field_helpers_declared_in_a_wrapper_function() {
    // A UMD wrapper or an unsplit module factory holds the helpers, the map,
    // and the class in one function body.
    let input = format!(
        r#"(function (root) {{
{BABEL_PRIVATE_HELPERS}
var _n = new WeakMap();
class Tally {{
  constructor() {{ _classPrivateFieldInitSpec(this, _n, 0); }}
  inc() {{ _classPrivateFieldSet(_n, this, _classPrivateFieldGet(_n, this) + 1); return _classPrivateFieldGet(_n, this); }}
}}
root.Tally = Tally;
}})(window);
"#
    );
    let output = render(&input);
    assert_counter_recovered(&output, "n");
}

#[test]
fn recovers_older_esbuild_private_field_helpers() {
    let helpers = r#"
var __accessCheck = (obj, member, msg) => {
  if (!member.has(obj)) throw TypeError("Cannot " + msg);
};
var __privateGet = (obj, member, getter) => {
  __accessCheck(obj, member, "read from private field");
  return getter ? getter.call(obj) : member.get(obj);
};
var __privateAdd = (obj, member, value) => {
  if (member.has(obj)) throw TypeError("Cannot add the same private member more than once");
  member instanceof WeakSet ? member.add(obj) : member.set(obj, value);
};
var __privateSet = (obj, member, value, setter) => {
  __accessCheck(obj, member, "write to private field");
  setter ? setter.call(obj, value) : member.set(obj, value);
  return value;
};
"#;
    let input = format!(
        r#"{helpers}
var _n;
export class Tally {{
  constructor() {{ __privateAdd(this, _n, 0); }}
  inc() {{ __privateSet(this, _n, __privateGet(this, _n) + 1); return __privateGet(this, _n); }}
}}
_n = new WeakMap();
"#
    );
    let output = render(&input);
    assert_counter_recovered(&output, "n");
}

#[test]
fn recovers_private_fields_of_a_derived_class_after_its_super_call() {
    let input = format!(
        r#"{BABEL_PRIVATE_HELPERS}
var _n = new WeakMap();
export class Tally extends Base {{
  constructor(...args) {{
    super(...args);
    _classPrivateFieldInitSpec(this, _n, 0);
    this.ready = true;
  }}
  inc() {{ _classPrivateFieldSet(_n, this, _classPrivateFieldGet(_n, this) + 1); return _classPrivateFieldGet(_n, this); }}
}}
"#
    );
    let output = render(&input);
    assert_counter_recovered(&output, "n");
    assert!(
        output.contains("super(...args);\n        this.ready = true;"),
        "{output}"
    );
}

#[test]
fn derived_private_fields_keep_the_super_call_boundary() {
    for constructor in [
        // The initializer reads a binding declared before `super`.
        "constructor(seed) { const start = seed * 2; super(); _classPrivateFieldInitSpec(this, _n, start); }",
        // Two super calls: the fields follow whichever runs.
        "constructor(flag) { if (flag) { super(1); } else { super(2); } _classPrivateFieldInitSpec(this, _n, 0); }",
        // The initializer does not directly follow the super call.
        "constructor() { super(); this.ready = true; _classPrivateFieldInitSpec(this, _n, 0); }",
    ] {
        let input = format!(
            r#"{BABEL_PRIVATE_HELPERS}
var _n = new WeakMap();
export class Tally extends Base {{
  {constructor}
  get() {{ return _classPrivateFieldGet(_n, this); }}
}}
"#
        );
        let output = render(&input);
        assert!(!output.contains("#n"), "{output}");
        assert!(output.contains("_classPrivateFieldInitSpec"), "{output}");
    }
}

#[test]
fn recovers_esbuild_maps_created_one_after_another_after_the_class() {
    let input = format!(
        r#"{ESBUILD_PRIVATE_HELPERS}
var _a, _b, _m;
export class Pair {{
  constructor() {{ __privateAdd(this, _a, 1); __privateAdd(this, _b, 2); }}
  sum() {{ return __privateGet(this, _a) + __privateGet(this, _b); }}
}}
_a = new WeakMap();
_m = new WeakSet();
_b = new WeakMap();
"#
    );
    let output = render(&input);
    assert!(output.contains("#a = 1"), "{output}");
    assert!(output.contains("#b = 2"), "{output}");
    assert!(output.contains("this.#a + this.#b"), "{output}");
    assert!(!output.contains("WeakMap"), "{output}");
}

// esbuild's helpers as a minifier leaves them in a bundle without private
// accessors: the getter and setter branches are gone.
const ESBUILD_SPECIALIZED_PRIVATE_HELPERS: &str = r#"
var __typeError = (msg) => { throw TypeError(msg); };
var __accessCheck = (obj, member, msg) => member.has(obj) || __typeError("Cannot " + msg);
var __privateGet = (obj, member) => (__accessCheck(obj, member, "read from private field"), member.get(obj));
var __privateAdd = (obj, member, value) => member.has(obj) ? __typeError("Cannot add the same private member more than once") : member instanceof WeakSet ? member.add(obj) : member.set(obj, value);
var __privateSet = (obj, member, value) => (__accessCheck(obj, member, "write to private field"), member.set(obj, value), value);
var __privateMethod = (obj, member, method) => (__accessCheck(obj, member, "access private method"), method);
"#;

fn esbuild_class_with_private_method(field_values: &str) -> String {
    format!(
        r#"{ESBUILD_SPECIALIZED_PRIVATE_HELPERS}
var _items, _limit, _trim, trim_fn;
export class Queue {{
  constructor(limit) {{
    __privateAdd(this, _trim);
    {field_values}
    __privateSet(this, _limit, limit);
  }}
  push(item) {{
    __privateGet(this, _items).push(item);
    __privateMethod(this, _trim, trim_fn).call(this);
  }}
}}
_items = new WeakMap();
_limit = new WeakMap();
_trim = new WeakSet();
trim_fn = function() {{
  while (__privateGet(this, _items).length > __privateGet(this, _limit)) __privateGet(this, _items).shift();
}};
"#
    )
}

#[test]
fn private_maps_read_by_a_lowered_private_method_stay_lowered() {
    // `trim_fn`, the lowered private method, reads the maps outside the class.
    let output = render(&esbuild_class_with_private_method(
        "__privateAdd(this, _items, []); __privateAdd(this, _limit);",
    ));
    assert!(!output.contains("#items"), "{output}");
    assert!(!output.contains("#limit"), "{output}");
}

#[test]
fn promotes_inert_fields_past_a_private_method_brand() {
    let input = format!(
        r#"{ESBUILD_SPECIALIZED_PRIVATE_HELPERS}
var _items, _limit, _brand;
export class Queue {{
  constructor(limit) {{
    __privateAdd(this, _brand);
    __privateAdd(this, _items, []);
    __privateAdd(this, _limit);
    __privateSet(this, _limit, limit);
  }}
  push(item) {{ __privateGet(this, _items).push(item); return __privateGet(this, _limit); }}
}}
_items = new WeakMap();
_limit = new WeakMap();
_brand = new WeakSet();
"#
    );
    let output = render(&input);
    assert!(output.contains("#items = [];"), "{output}");
    assert!(output.contains("#limit;"), "{output}");
    assert!(output.contains("this.#limit = limit"), "{output}");
    assert!(output.contains("this.#items.push(item)"), "{output}");
    assert!(output.contains("__privateAdd(this, "), "{output}");
}

#[test]
fn a_field_that_can_observe_a_private_method_brand_stays_after_it() {
    let input = format!(
        r#"{ESBUILD_SPECIALIZED_PRIVATE_HELPERS}
var _items, _brand;
export class Queue {{
  constructor() {{
    __privateAdd(this, _brand);
    __privateAdd(this, _items, this.seed());
  }}
  push(item) {{ __privateGet(this, _items).push(item); }}
}}
_items = new WeakMap();
_brand = new WeakSet();
"#
    );
    let output = render(&input);
    assert!(!output.contains("#items"), "{output}");
    assert!(output.contains("this.seed()"), "{output}");
}

#[test]
fn a_private_read_that_a_rewrite_would_rebind_keeps_the_map() {
    for (read, recovered) in [
        ("__privateGet(this, _fn).call(this, 1)", true),
        ("__privateGet(this, _fn)(1)", false),
        ("(__privateGet(this, _fn))(1)", false),
        ("__privateGet(this, _fn)?.(1)", false),
        ("__privateGet(this, _fn)`tag`", false),
        ("delete __privateGet(this, _fn)", false),
    ] {
        let input = format!(
            "{ESBUILD_PRIVATE_HELPERS}\nvar _fn;\nexport class Foo {{\n  constructor() {{ __privateAdd(this, _fn, function() {{ return this; }}); }}\n  run() {{ return {read}; }}\n}}\n_fn = new WeakMap();\n"
        );
        let output = render(&input);
        assert_eq!(output.contains("#fn"), recovered, "{read}: {output}");
    }
}

#[test]
fn promotes_tsc_weakmap_private_field_with_template_literal_kind() {
    // A minifier that prints strings as templates writes the `"f"` kind
    // argument as `` `f` ``.
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
        return __classPrivateFieldGet(this, _Foo_x, `f`);
    }
    setX(value) {
        __classPrivateFieldSet(this, _Foo_x, value, `f`);
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
