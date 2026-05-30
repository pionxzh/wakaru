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
