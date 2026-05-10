mod common;

use common::assert_eq_normalized;
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

// ============================================================
// Basic: function + prototype methods → class
// ============================================================

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
