mod common;

use common::{assert_eq_normalized, render, render_pipeline_until, render_rule};
use swc_core::common::GLOBALS;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::rules::UnPrototypeClass;

fn apply(input: &str) -> String {
    GLOBALS.set(&Default::default(), || {
        use swc_core::common::{sync::Lrc, FileName, SourceMap};
        use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
        use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("test.js".to_string()).into(),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax {
                jsx: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("parse failed");

        module.visit_mut_with(&mut UnPrototypeClass);

        let mut output = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: Config::default().with_minify(false),
                cm: cm.clone(),
                comments: None,
                wr: JsWriter::new(cm, "\n", &mut output, None),
            };
            emitter.emit_module(&module).expect("emit failed");
        }
        String::from_utf8(output).expect("utf-8")
    })
}

fn apply_resolved(input: &str) -> String {
    render_rule(input, |_| UnPrototypeClass)
}

// ============================================================
// Basic: function + prototype methods → class
// ============================================================

#[test]
fn duplicate_constructor_params_preserve_prototype_shape() {
    let input = r#"
function Foo(a, a) {
    this.value = a;
}
Foo.prototype.run = function() {
    return this.value;
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn duplicate_method_params_preserve_prototype_shape() {
    let input = r#"
function Foo(value) {
    this.value = value;
}
Foo.prototype.pick = function(a, a) {
    return a;
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn test_basic_prototype_class() {
    let input = r#"
function Foo(name) {
    this.name = name;
}
Foo.prototype.greet = function() {
    return "hello " + this.name;
};
Foo.prototype.getName = function() {
    return this.name;
};
"#;
    let expected = r#"
class Foo {
    constructor(name) {
        this.name = name;
    }
    greet() {
        return "hello " + this.name;
    }
    getName() {
        return this.name;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Empty constructor is omitted
// ============================================================

#[test]
fn test_empty_constructor_omitted() {
    let input = r#"
function Foo() {}
Foo.prototype.run = function() { return true; };
"#;
    let expected = r#"
class Foo {
    run() { return true; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Foo.prototype.constructor = Foo is skipped
// ============================================================

#[test]
fn test_prototype_constructor_skipped() {
    let input = r#"
function Foo(x) { this.x = x; }
Foo.prototype.constructor = Foo;
Foo.prototype.getX = function() { return this.x; };
"#;
    let expected = r#"
class Foo {
    constructor(x) { this.x = x; }
    getX() { return this.x; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Static methods: Foo.staticMethod = function() {}
// ============================================================

#[test]
fn test_static_methods() {
    let input = r#"
function Foo() {}
Foo.prototype.run = function() {};
Foo.create = function(x) { return new Foo(x); };
"#;
    let expected = r#"
class Foo {
    run() {}
    static create(x) { return new Foo(x); }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Interleaved non-method statements are preserved
// ============================================================

#[test]
fn test_interleaved_statements() {
    let input = r#"
function Foo() {}
Foo.prototype.a = function() { return 1; };
const x = 42;
Foo.prototype.b = function() { return 2; };
"#;
    let expected = r#"
class Foo {
    a() { return 1; }
    b() { return 2; }
}
const x = 42;
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Inheritance via Object.create
// ============================================================

#[test]
fn test_inheritance_object_create() {
    let input = r#"
function Child(name) {
    Parent.call(this, name);
}
Child.prototype = Object.create(Parent.prototype);
Child.prototype.constructor = Child;
Child.prototype.speak = function() { return "hi"; };
"#;
    let expected = r#"
class Child extends Parent {
    constructor(name) {
        super(name);
    }
    speak() { return "hi"; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Inheritance via util.inherits
// ============================================================

#[test]
fn test_inheritance_util_inherits() {
    let input = r#"
function Child(name) {
    Parent.call(this, name);
}
util.inherits(Child, Parent);
Child.prototype.speak = function() { return "hi"; };
"#;
    let expected = r#"
class Child extends Parent {
    constructor(name) {
        super(name);
    }
    speak() { return "hi"; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_closure_function_expression_classes() {
    // Shape reaching this rule after VarDeclToLetConst normalizes Closure
    // Compiler's safe single-declarator `var` bindings.
    let input = r#"
const Base = function() {};
Base.prototype.greet = function() { return "hi"; };
const Child = function(name) {
    Base.call(this);
    this.name = name;
};
$jscomp.inherits(Child, Base);
Child.prototype.label = function() {
    return this.greet() + " " + this.name;
};
"#;
    let expected = r#"
class Base {
    greet() { return "hi"; }
}
class Child extends Base {
    constructor(name) {
        super();
        this.name = name;
    }
    label() {
        return this.greet() + " " + this.name;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_closure_function_expression_classes_in_pipeline() {
    let input = r#"
var Base = function() {};
Base.prototype.greet = function() { return "hi"; };
var Child = function(name) {
    Base.call(this);
    this.name = name;
};
$jscomp.inherits(Child, Base);
Child.prototype.label = function() { return this.name; };
window.Child = Child;
"#;
    let expected = r#"
class Base {
    greet() { return "hi"; }
}
class Child extends Base {
    constructor(name) {
        super();
        this.name = name;
    }
    label() { return this.name; }
}
window.Child = Child;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn test_function_expression_class_skips_multiple_declarators() {
    let input = r#"
var Foo = function() {}, unrelated = sideEffect();
Foo.prototype.run = function() { return true; };
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn test_function_expression_class_skips_pre_reference() {
    let input = r#"
use(Foo);
const Foo = function() { this.value = 1; };
Foo.prototype.run = function() { return this.value; };
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn function_expression_class_preserves_block_escaping_var() {
    let input = r#"
function demo(flag) {
    if (flag) {
        var Foo = function() {};
        Foo.prototype.run = function() { return true; };
    }
    return Foo;
}
"#;
    let before = render_pipeline_until(input, "ObjMethodShorthand");
    let after = render_pipeline_until(input, "UnPrototypeClass");
    assert_eq_normalized(&after, &before);
}

#[test]
fn function_expression_class_preserves_var_observed_by_earlier_closure() {
    let input = r#"
function readFoo() { return Foo; }
consume(readFoo());
var Foo = function() {};
Foo.prototype.run = function() { return true; };
"#;
    let before = render_pipeline_until(input, "ObjMethodShorthand");
    let after = render_pipeline_until(input, "UnPrototypeClass");
    assert_eq_normalized(&after, &before);
}

#[test]
fn function_expression_class_ignores_shadowed_pre_reference() {
    let input = r#"
function unrelated(Foo) { return Foo; }
const Foo = function() {};
Foo.prototype.run = function() { return true; };
"#;
    let expected = r#"
function unrelated(Foo) { return Foo; }
class Foo {
    run() { return true; }
}
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn function_expression_class_preserves_unrecognized_interleaved_inheritance() {
    let input = r#"
const Base = function() {};
Base.prototype.base = function() { return true; };
const Child = function() {};
runtime.attachBase(Child, Base);
Child.prototype.run = function() { return this.base(); };
"#;
    let expected = r#"
class Base {
    base() { return true; }
}
const Child = function() {};
runtime.attachBase(Child, Base);
Child.prototype.run = function() { return this.base(); };
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn function_declaration_keeps_recovery_across_define_property_call() {
    let input = r#"
function RecordType() {}
Object.defineProperty(RecordType.prototype, "value", {
    get: makeGetter()
});
RecordType.prototype.serialize = function() { return this.value; };
"#;
    let expected = r#"
class RecordType {
    serialize() { return this.value; }
}
Object.defineProperty(RecordType.prototype, "value", {
    get: makeGetter()
});
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

#[test]
fn function_declaration_keeps_recovery_across_define_properties_call() {
    let input = r#"
function TreeNode() {}
Object.defineProperties(TreeNode.prototype, {
    owner: {
        get: function() { return this; }
    }
});
TreeNode.prototype.serialize = function() { return this.owner; };
"#;
    let expected = r#"
class TreeNode {
    serialize() { return this.owner; }
}
Object.defineProperties(TreeNode.prototype, {
    owner: {
        get: function() { return this; }
    }
});
"#;
    assert_eq_normalized(&apply_resolved(input), expected);
}

// ============================================================
// No-op: function without prototype methods
// ============================================================

#[test]
fn test_noop_no_prototype_methods() {
    let input = r#"
function helper(x) {
    this.x = x;
}
const y = helper(1);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// ============================================================
// No-op: regular function (no `this`)
// ============================================================

#[test]
fn test_noop_no_this() {
    let input = r#"
function add(a, b) {
    return a + b;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// ============================================================
// Multiple classes in same scope
// ============================================================

#[test]
fn test_multiple_classes() {
    let input = r#"
function Foo() { this.x = 1; }
Foo.prototype.getX = function() { return this.x; };
function Bar() { this.y = 2; }
Bar.prototype.getY = function() { return this.y; };
"#;
    let expected = r#"
class Foo {
    constructor() { this.x = 1; }
    getX() { return this.x; }
}
class Bar {
    constructor() { this.y = 2; }
    getY() { return this.y; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Non-function prototype assignment is NOT consumed
// ============================================================

#[test]
fn test_non_function_prototype_left_alone() {
    let input = r#"
function Foo() {}
Foo.prototype.run = function() {};
Foo.prototype.isReactComponent = {};
"#;
    let expected = r#"
class Foo {
    run() {}
}
Foo.prototype.isReactComponent = {};
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Getter/setter via Object.defineProperty
// ============================================================

#[test]
fn test_getter_setter() {
    let input = r#"
function Foo(val) { this._val = val; }
Object.defineProperty(Foo.prototype, "value", {
    get: function() { return this._val; },
    set: function(v) { this._val = v; }
});
"#;
    let expected = r#"
class Foo {
    constructor(val) { this._val = val; }
    get value() { return this._val; }
    set value(v) { this._val = v; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn duplicate_getter_params_preserve_define_property_shape() {
    let input = r#"
function Foo(val) { this._val = val; }
Object.defineProperty(Foo.prototype, "value", {
    get: function(a, a) { return this._val; }
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn test_define_property_value_function_method() {
    let input = r#"
function Foo(val) { this._val = val; }
Object.defineProperty(Foo.prototype, "value", {
    enumerable: false,
    configurable: true,
    writable: true,
    value: function value() { return this._val; }
});
"#;
    let expected = r#"
class Foo {
    constructor(val) { this._val = val; }
    value() { return this._val; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn duplicate_value_method_params_preserve_define_property_shape() {
    let input = r#"
function Foo(val) { this._val = val; }
Object.defineProperty(Foo.prototype, "value", {
    enumerable: false,
    configurable: true,
    writable: true,
    value: function value(a, a) { return a; }
});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn test_define_property_value_function_enumerable_true_not_method() {
    let input = r#"
function Foo() {}
Object.defineProperty(Foo.prototype, "value", {
    enumerable: true,
    configurable: true,
    writable: true,
    value: function value() { return 1; }
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// ============================================================
// Pre-reference relocation (contiguous prelude only)
// ============================================================

#[test]
fn test_pre_ref_module_exports() {
    let input = r#"
module.exports = Foo;
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let expected = r#"
class Foo {
    constructor(x) { this.x = x; }
    getX() { return this.x; }
}
module.exports = Foo;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_pre_ref_esbuild_commonjs_param() {
    let input = r#"
fn4.exports = Foo;
Foo.className = "SyntheticType";
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let expected = r#"
class Foo {
    constructor(x) { this.x = x; }
    getX() { return this.x; }
}
fn4.exports = Foo;
Foo.className = "SyntheticType";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_pre_ref_exports_default() {
    let input = r#"
exports.default = Foo;
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let expected = r#"
class Foo {
    constructor(x) { this.x = x; }
    getX() { return this.x; }
}
exports.default = Foo;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_pre_ref_with_gap() {
    // Real esbuild pattern: export + className at top, const/let in between,
    // fn decl much later. Safe patterns are relocated regardless of distance.
    let input = r#"
fn4.exports = Foo;
Foo.className = "SyntheticType";
const x = 1;
const y = 2;
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let expected = r#"
const x = 1;
const y = 2;
class Foo {
    constructor(x) { this.x = x; }
    getX() { return this.x; }
}
fn4.exports = Foo;
Foo.className = "SyntheticType";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_pre_ref_export_with_gap() {
    let input = r#"
module.exports = Foo;
sideEffect();
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let expected = r#"
sideEffect();
class Foo {
    constructor(x) { this.x = x; }
    getX() { return this.x; }
}
module.exports = Foo;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_pre_ref_arbitrary_call_skips() {
    let input = r#"
use(Foo);
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn test_pre_ref_call_in_lhs_skips() {
    let input = r#"
getTarget().value = Foo;
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn test_pre_ref_bare_ident_assignment_skips() {
    let input = r#"
x = Foo;
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn test_pre_ref_mixed_safe_and_unsafe_skips() {
    let input = r#"
module.exports = Foo;
console.log(Foo);
function Foo(x) { this.x = x; }
Foo.prototype.getX = function() { return this.x; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// ============================================================
// Chained inheritance (protobuf.js codegen pattern)
// ============================================================

#[test]
fn test_chained_inheritance_with_classname() {
    let input = r#"
mod1.exports = Foo;
((Foo.prototype = Object.create(Bar.prototype)).constructor = Foo).className = "Root";
function Foo(x) {
    Bar.call(this, x);
}
Foo.prototype.getX = function() { return this.x; };
"#;
    let expected = r#"
class Foo extends Bar {
    constructor(x) {
        super(x);
    }
    getX() { return this.x; }
}
mod1.exports = Foo;
Foo.className = "Root";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_chained_inheritance_without_classname() {
    let input = r#"
mod1.exports = Foo;
(Foo.prototype = Object.create(Bar.prototype)).constructor = Foo;
function Foo(x) {
    Bar.call(this, x);
}
Foo.prototype.getX = function() { return this.x; };
"#;
    let expected = r#"
class Foo extends Bar {
    constructor(x) {
        super(x);
    }
    getX() { return this.x; }
}
mod1.exports = Foo;
"#;
    assert_eq_normalized(&apply(input), expected);
}
