mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::ObjMethodShorthand;

fn apply(input: &str) -> String {
    render_rule(input, ObjMethodShorthand::new)
}

#[test]
fn function_value_becomes_method_shorthand() {
    let input = r#"
const obj = {
    greet: function(name) {
        return "hello " + name;
    }
};
"#;
    let expected = r#"
const obj = {
    greet(name) {
        return "hello " + name;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn constructible_object_property_stays_function_value() {
    let input = r#"
const namespace = {
    Constructor: function(value) {
        this.value = value;
    }
};
new namespace.Constructor(input);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn create_class_argument_property_stays_function_value() {
    // createClass defines methods on its first argument's prototype.
    let input = r#"
function e(r, e, n) {
    return e && t(r.prototype, e), n && t(r, n), Object.defineProperty(r, "prototype", { writable: !1 }), r;
}
const ns = {C: function() {}};
e(ns.C, []);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn constructible_object_property_stays_function_through_object_alias() {
    let input = r#"
const namespace = {
    Constructor: function(value) {
        this.value = value;
    }
};
const alias = namespace;
new alias.Constructor(input);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructible_object_property_stays_function_in_wrapped_object_values() {
    let inputs = [
        r#"
const namespace = (sideEffect(), {
    Constructor: function(value) { this.value = value; }
});
new namespace.Constructor(input);
"#,
        r#"
const namespace = cached || {
    Constructor: function(value) { this.value = value; }
};
new namespace.Constructor(input);
"#,
        r#"
const namespace = {
    ...{ Constructor: function(value) { this.value = value; } }
};
new namespace.Constructor(input);
"#,
        r#"
const namespace = condition ? {
    nested: { Constructor: function(value) { this.value = value; } }
} : fallback;
new namespace.nested.Constructor(input);
"#,
        r#"
let namespace;
namespace = {
    Constructor: function(value) { this.value = value; }
};
new namespace.Constructor(input);
"#,
    ];

    for input in inputs {
        let output = apply(input);
        assert!(output.contains("Constructor: function(value)"), "{output}");
    }
}

#[test]
fn constructible_object_property_stays_function_under_conditional_new_callee() {
    let input = r#"
const namespace = {
    Constructor: function(value) {
        this.value = value;
    }
};
new (condition ? namespace.Constructor : fallback)(input);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructible_object_property_stays_function_through_destructured_alias() {
    let input = r#"
const namespace = {
    Constructor: function(value) {
        this.value = value;
    }
};
const { Constructor } = namespace;
new Constructor(input);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructible_inline_object_property_stays_function_when_destructured() {
    let inputs = [
        r#"
const { Constructor } = {
    Constructor: function(value) {
        this.value = value;
    }
};
new Constructor(input);
"#,
        r#"
let Constructor;
({ Constructor } = {
    Constructor: function(value) {
        this.value = value;
    }
});
new Constructor(input);
"#,
    ];

    for input in inputs {
        let output = apply(input);
        assert_eq_normalized(&output, input);
    }
}

#[test]
fn exported_destructured_object_property_stays_constructible() {
    let input = r#"
const namespace = {
    Constructor: function(value) {
        this.value = value;
    }
};
export const { Constructor } = namespace;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructible_object_property_stays_function_in_destructured_default() {
    let inputs = [
        r#"
const { namespace = {
    Constructor: function(value) {
        this.value = value;
    }
} } = {};
new namespace.Constructor(input);
"#,
        r#"
let namespace;
({ namespace = {
    Constructor: function(value) {
        this.value = value;
    }
} } = {});
new namespace.Constructor(input);
"#,
    ];

    for input in inputs {
        let output = apply(input);
        assert_eq_normalized(&output, input);
    }
}

#[test]
fn constructible_object_property_stays_function_in_logical_assignment_value() {
    let inputs = [
        r#"
let namespace;
namespace ||= {
    Constructor: function(value) {
        this.value = value;
    }
};
new namespace.Constructor(input);
"#,
        r#"
let namespace;
namespace ??= {
    Constructor: function(value) {
        this.value = value;
    }
};
new namespace.Constructor(input);
"#,
    ];

    for input in inputs {
        let output = apply(input);
        assert_eq_normalized(&output, input);
    }
}

#[test]
fn duplicate_params_stay_key_value_function() {
    // Method parameter lists require unique names (UniqueFormalParameters);
    // a sloppy-mode function expression may carry duplicates.
    let input = r#"
const obj = {
    pick: function (a, a) {
        return a;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn multiple_methods_converted() {
    let input = r#"
const obj = {
    a: function() { return 1; },
    b: function(x) { return x * 2; }
};
"#;
    let expected = r#"
const obj = {
    a() { return 1; },
    b(x) { return x * 2; }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn anonymous_function_with_different_key_becomes_shorthand() {
    // When the function has no internal name, conversion is safe regardless of key name
    let input = r#"
const obj = {
    foo: function() {
        return 1;
    }
};
"#;
    let expected = r#"
const obj = {
    foo() {
        return 1;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_method_not_converted() {
    // Generator functions cannot be expressed as method shorthand without `*` —
    // keep as key-value pair to avoid changing semantics
    let input = r#"
const obj = {
    gen: function* () {
        yield 1;
    }
};
"#;
    let expected = r#"
const obj = {
    gen: function* () {
        yield 1;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_function_expr_not_converted() {
    // A named function expression may reference itself by name inside the body.
    // Converting to shorthand would drop that internal name, breaking recursion.
    let input = r#"
const x = {foo: function foo() { return foo(); }};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn computed_key_not_converted() {
    // Computed property keys are dynamic — shorthand syntax does not support them
    let input = r#"
const x = {[foo]: function() {}};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn string_key_not_converted() {
    // String-keyed properties cannot use method shorthand syntax
    let input = r#"
const x = {"foo": function() {}};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn numeric_key_not_converted() {
    // Numeric-keyed properties cannot use method shorthand syntax
    let input = r#"
const x = {123: function() {}};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn call_result_constructed_member_stays_function() {
    // call_result_exposes_argument_properties: `new Word.init` constructs the
    // `init` copied from the object passed to `extend`. Sibling methods are
    // not constructed and stay eligible for shorthand.
    let input = r#"
function extend(props) {
    return props;
}
var Word = extend({
    init: function(hi, lo) {
        this.hi = hi;
    },
    describe: function() {
        return this.hi;
    }
});
new Word.init(1, 2);
"#;
    let expected = r#"
function extend(props) {
    return props;
}
var Word = extend({
    init: function(hi, lo) {
        this.hi = hi;
    },
    describe() {
        return this.hi;
    }
});
new Word.init(1, 2);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn chained_assignment_call_result_constructed_member_stays_function() {
    let input = r#"
var ns = {};
var c = ns.Word = extend({
    init: function() {},
    other: function() {}
});
new c.init();
"#;
    let expected = r#"
var ns = {};
var c = ns.Word = extend({
    init: function() {},
    other() {}
});
new c.init();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn intermediate_binding_call_result_constructed_member_stays_function() {
    // Binding-to-binding fixpoint must run before the member-alias step.
    // `new d.init` reaches `ns.Word.init` via `d = c` and `c = ns.Word`.
    let input = r#"
var ns = {};
var c = ns.Word = extend({
    init: function() {},
    other: function() {}
});
var d = c;
new d.init();
"#;
    let expected = r#"
var ns = {};
var c = ns.Word = extend({
    init: function() {},
    other() {}
});
var d = c;
new d.init();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn wrapped_call_result_constructed_member_stays_function() {
    let cases = [
        (
            r#"
var Word = (sideEffect(), extend({
    init: function() {},
    other: function() {}
}));
new Word.init();
"#,
            r#"
var Word = (sideEffect(), extend({
    init: function() {},
    other() {}
}));
new Word.init();
"#,
        ),
        (
            r#"
var Word = (extend({
    init: function() {},
    other: function() {}
}));
new Word.init();
"#,
            r#"
var Word = extend({
    init: function() {},
    other() {}
});
new Word.init();
"#,
        ),
        (
            r#"
var Word = fallback || extend({
    init: function() {},
    other: function() {}
});
new Word.init();
"#,
            r#"
var Word = fallback || extend({
    init: function() {},
    other() {}
});
new Word.init();
"#,
        ),
    ];
    for (input, expected) in cases {
        assert_eq_normalized(&apply(input), expected);
    }
}

#[test]
fn call_result_without_construct_still_uses_method_shorthand() {
    let input = r#"
var Word = extend({
    init: function() {},
    other: function() {}
});
Word.init();
"#;
    let expected = r#"
var Word = extend({
    init() {},
    other() {}
});
Word.init();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn call_result_ident_argument_is_not_linked() {
    // Accepted boundary: only an inline object argument is linked to the call
    // result. An argument binding is not followed, so `existing.init` still
    // converts even though `extend` may expose it as `c.init`.
    let input = r#"
const existing = {
    init: function() {},
    other: function() {}
};
var c = extend(existing);
new c.init();
"#;
    let expected = r#"
const existing = {
    init() {},
    other() {}
};
var c = extend(existing);
new c.init();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn spread_argument_does_not_link_call_result_members() {
    let input = r#"
var Word = extend(...items);
var other = extend({
    init: function() {},
    other: function() {}
});
new Word.init();
"#;
    let expected = r#"
var Word = extend(...items);
var other = extend({
    init() {},
    other() {}
});
new Word.init();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn computed_key_in_call_argument_does_not_freeze_siblings() {
    let input = r#"
var Word = extend({
    [name]: function() {},
    other: function() {}
});
new Word.init();
"#;
    let expected = r#"
var Word = extend({
    [name]: function() {},
    other() {}
});
new Word.init();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_object_init_still_uses_method_shorthand() {
    // The constructed `init` is the call-argument property. A nested object
    // inside that function is a different value.
    let input = r#"
var Word = extend({
    init: function() {
        const box = {
            init: function() {
                return 1;
            }
        };
        return box;
    },
    other: function() {}
});
new Word.init();
"#;
    let expected = r#"
var Word = extend({
    init: function() {
        const box = {
            init() {
                return 1;
            }
        };
        return box;
    },
    other() {}
});
new Word.init();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn unconstructed_call_argument_still_uses_method_shorthand() {
    // Another module may evaluate `new Word.init`. This module never does,
    // so both properties stay eligible for method shorthand.
    let input = r#"
var Word = extend({
    init: function() {},
    other: function() {}
});
exportWord(Word);
"#;
    let expected = r#"
var Word = extend({
    init() {},
    other() {}
});
exportWord(Word);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn receiver_constructed_member_stays_function() {
    // call_result_exposes_argument_properties: a one-argument mixin copies
    // the object onto its receiver, so `make` becomes `Lib.make`, which this
    // module constructs.
    let input = r#"
Lib.mixin({
    items: [],
    make: function(first, second) {
        this.first = first;
    },
    size: function(value) {
        return value;
    }
});
Lib.make.prototype = {};
new Lib.make(alpha, beta);
"#;
    let expected = r#"
Lib.mixin({
    items: [],
    make: function(first, second) {
        this.first = first;
    },
    size (value) {
        return value;
    }
});
Lib.make.prototype = {};
new Lib.make(alpha, beta);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn receiver_without_construct_still_uses_method_shorthand() {
    let input = r#"
Lib.mixin({
    make: function(first) {
        this.first = first;
    }
});
Lib.make(alpha);
"#;
    let expected = r#"
Lib.mixin({
    make (first) {
        this.first = first;
    }
});
Lib.make(alpha);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_receiver_constructed_member_stays_function() {
    let input = r#"
Lib.proto.mixin({
    start: function(value) {
        this.value = value;
    },
    other: function() {}
});
new Lib.proto.start("a");
"#;
    let expected = r#"
Lib.proto.mixin({
    start: function(value) {
        this.value = value;
    },
    other () {}
});
new Lib.proto.start("a");
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn call_result_and_receiver_are_both_checked() {
    // The argument object is linked to the assigned result and to the
    // receiver at once; constructing either one keeps the property.
    let input = r#"
var Word = Base.extend({
    init: function() {},
    create: function() {},
    other: function() {}
});
new Word.init();
new Base.create();
"#;
    let expected = r#"
var Word = Base.extend({
    init: function() {},
    create: function() {},
    other () {}
});
new Word.init();
new Base.create();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn constructor_key_stays_function() {
    // Class-system helpers return a `constructor` property as the class, and
    // `new this.constructor()` constructs one read from a prototype object.
    // The construction is usually invisible to this module.
    let input = r#"
const Item = extend(Base, {
    constructor: function(first, second) {
        Base.call(this, first, second);
    },
    other: function() {
        return 1;
    }
});
Shape.prototype = {
    constructor: function(value) {
        this.value = value;
    }
};
"#;
    let expected = r#"
const Item = extend(Base, {
    constructor: function(first, second) {
        Base.call(this, first, second);
    },
    other() {
        return 1;
    }
});
Shape.prototype = {
    constructor: function(value) {
        this.value = value;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn prototype_of_assignment_result_stays_constructible() {
    // wild-observed: a minified Tween class assigns its constructor to a
    // namespace inside the prototype target.
    let input = r#"
var T = {};
function E(e) {
    return new E.prototype.init(e);
}
((T.Tween = E).prototype = {
    constructor: E,
    init: function(e) {
        this.e = e;
    }
}).init.prototype = E.prototype;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn chained_prototype_target_stays_constructible() {
    // wild-observed: `S.fn = S.prototype = { init }` with `new S.fn.init()`.
    let input = r#"
var S = function(e) {
    return new S.fn.init(e);
};
S.fn = S.prototype = {
    init: function(e) {
        this.e = e;
    },
    size: function() {
        return 1;
    }
};
"#;
    let expected = r#"
var S = function(e) {
    return new S.fn.init(e);
};
S.fn = S.prototype = {
    init: function(e) {
        this.e = e;
    },
    size() {
        return 1;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn constructed_member_suffix_in_sibling_scope_stays_function() {
    // wild-observed: UMD files that share a namespace only through a global
    // (`C.algo` in one IIFE, `CryptoJS.algo` in the next), so the resolver
    // sees two unrelated roots.
    let input = r#"
(function() {
    var C_algo = CryptoJS.algo = {};
    CryptoJS.hmac = function(h, k) {
        return new C_algo.HMAC.init(h, k);
    };
})();
(function() {
    var C_algo = CryptoJS.algo;
    C_algo.HMAC = Base.extend({
        init: function(h, k) {
            this.h = h;
        },
        reset: function() {
            this.h = null;
        }
    });
})();
"#;
    let expected = r#"
(function() {
    var C_algo = CryptoJS.algo = {};
    CryptoJS.hmac = function(h, k) {
        return new C_algo.HMAC.init(h, k);
    };
})();
(function() {
    var C_algo = CryptoJS.algo;
    C_algo.HMAC = Base.extend({
        init: function(h, k) {
            this.h = h;
        },
        reset() {
            this.h = null;
        }
    });
})();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn constructed_member_suffix_ignores_prototype_parent_and_other_parents() {
    // `prototype.init` is shared by unrelated classes, and `Other.init`
    // differs in the parent name.
    let input = r#"
function A() {}
new A.prototype.init();
var B = function() {};
B.prototype = {
    init: function() {}
};
var ns = {};
new ns.HMAC.init();
other.Other = make({
    init: function() {}
});
"#;
    let expected = r#"
function A() {}
new A.prototype.init();
var B = function() {};
B.prototype = {
    init() {}
};
var ns = {};
new ns.HMAC.init();
other.Other = make({
    init() {}
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
