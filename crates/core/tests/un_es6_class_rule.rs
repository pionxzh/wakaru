mod common;

use common::assert_eq_normalized;
use swc_core::common::GLOBALS;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::rules::UnEs6Class;

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

        module.visit_mut_with(&mut UnEs6Class);

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
// Basic class with constructor and prototype method
// ============================================================

#[test]
fn test_basic_class_ts_output() {
    let input = r#"
var Foo = (function() {
    function t(name) { this.name = name; }
    t.prototype.logger = function logger() { console.log(this.name); }
    t.staticMethod = function staticMethod() { console.log('static'); }
    return t;
}());
"#;
    let expected = r#"
class Foo {
    constructor(name) { this.name = name; }
    logger() { console.log(this.name); }
    static staticMethod() { console.log('static'); }
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
var Bar = (function() {
    function t() {}
    t.prototype.greet = function greet() { return 'hello'; }
    return t;
}());
"#;
    let expected = r#"
class Bar {
    greet() { return 'hello'; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Inheritance via __extends
// ============================================================

#[test]
fn test_inheritance_extends() {
    let input = r#"
var Child = (function(_super) {
    __extends(t, _super);
    function t(name) { _super.call(this, name); }
    t.prototype.speak = function speak() { return 'hi'; }
    return t;
}(Animal));
"#;
    let expected = r#"
class Child extends Animal {
    constructor(name) { super(name); }
    speak() { return 'hi'; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_super_rewrite_skips_nested_function_scope() {
    let input = r#"
var Child = (function(e) {
    _inherits(t, e);
    function t() {
        e.call(this);
        function inner(e) {
            return e.call(this);
        }
        this.inner = inner;
    }
    return t;
}(Base));
"#;
    let output = apply(input);
    assert!(
        output.contains("super()"),
        "outer constructor super call should be rewritten: {output}"
    );
    assert!(
        output.contains("return e.call(this)"),
        "nested shadowed e.call(this) must remain unchanged: {output}"
    );
    assert!(
        !output.contains("return super()"),
        "nested function must not be rewritten to super(): {output}"
    );
}

// ============================================================
// Inheritance via _inherits (Babel)
// ============================================================

#[test]
fn test_inheritance_inherits() {
    let input = r#"
var Child = (function(_super) {
    _inherits(t, _super);
    function t() {}
    t.prototype.run = function run() {}
    return t;
}(Base));
"#;
    let expected = r#"
class Child extends Base {
    run() {}
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Getter and setter via Object.defineProperty
// ============================================================

#[test]
fn test_inheritance_member_expr_super() {
    // Super class is a member expression (e.g. React.Component or module.Component)
    let input = r#"
var Child = (function(_super) {
    _inherits(t, _super);
    function t() {}
    t.prototype.run = function run() {}
    return t;
}(module.Component));
"#;
    let expected = r#"
class Child extends module.Component {
    run() {}
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_getter_setter_define_property() {
    let input = r#"
var MyClass = (function() {
    function t(val) { this._val = val; }
    Object.defineProperty(t.prototype, "value", {
        get: function() { return this._val; },
        set: function(v) { this._val = v; }
    });
    return t;
}());
"#;
    let expected = r#"
class MyClass {
    constructor(val) { this._val = val; }
    get value() { return this._val; }
    set value(v) { this._val = v; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Babel loose mode: proto alias
// ============================================================

#[test]
fn test_babel_loose_proto_alias() {
    let input = r#"
var Greeter = (function() {
    function t(name) { this.name = name; }
    var proto = t.prototype;
    proto.greet = function greet() { return 'hi ' + this.name; }
    return t;
}());
"#;
    let expected = r#"
class Greeter {
    constructor(name) { this.name = name; }
    greet() { return 'hi ' + this.name; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Babel _createClass variant
// ============================================================

#[test]
fn test_babel_create_class() {
    let input = r#"
var MyClass = (function() {
    function t(x) { this.x = x; }
    return _createClass(t, [{
        key: "getX",
        value: function getX() { return this.x; }
    }], [{
        key: "create",
        value: function create(x) { return new t(x); }
    }]);
}());
"#;
    let expected = r#"
class MyClass {
    constructor(x) { this.x = x; }
    getX() { return this.x; }
    static create(x) { return new t(x); }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// No-op: not a class IIFE (should be left unchanged)
// ============================================================

#[test]
fn test_noop_not_a_class() {
    let input = r#"
var x = (function() {
    return 42;
}());
"#;
    // No inner function declaration → not a class
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// ============================================================
// No-op: prototype inheritance setup lines are skipped
// ============================================================

#[test]
fn test_prototype_chain_setup_skipped() {
    let input = r#"
var Child = (function(_super) {
    __extends(t, _super);
    function t() { _super.apply(this, arguments); }
    t.prototype = Object.create(_super.prototype);
    t.prototype.constructor = t;
    t.prototype.doSomething = function doSomething() { return true; }
    return t;
}(Parent));
"#;
    // _super.apply(this, arguments) → super(...arguments) → default ctor removed
    let expected = r#"
class Child extends Parent {
    doSomething() { return true; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Inlined inheritance (webpack4 pattern without _inherits)
// ============================================================

#[test]
fn test_inlined_inheritance_webpack4() {
    // webpack4 inlines the _inherits logic directly instead of calling _inherits
    let input = r#"
var Child = (function(_super) {
    if (typeof _super !== "function" && _super !== null) {
        throw new TypeError("Super expression must either be null or a function");
    }
    function t() {
        _super !== null && _super.apply(this, arguments);
    }
    t.prototype = Object.create(_super !== null && _super.prototype);
    t.prototype.constructor = t;
    _super && (Object.setPrototypeOf ? Object.setPrototypeOf(t, _super) : t.__proto__ = _super);
    t.prototype.run = function run() { return true; }
    return t;
}(Base));"#;
    // _super.apply(this, arguments) is rewritten to super(...arguments)
    let expected = r#"
class Child extends Base {
    constructor() {
        _super !== null && super(...arguments);
    }
    run() { return true; }
}"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Both call forms: (function(){...}()) and (function(){...})()
// ============================================================

#[test]
fn test_iife_call_form_outer_paren() {
    // (function() { ... })()  ← callee is paren-wrapped FnExpr
    let input = r#"
var A = (function() {
    function t() {}
    t.prototype.go = function go() {}
    return t;
})();
"#;
    let expected = r#"
class A {
    go() {}
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_arrow_iife_class_basic() {
    let input = r#"
var Foo = (() => {
    function t() {}
    t.prototype.render = function() {
        return null;
    };
    return t;
})();
"#;
    let expected = r#"
class Foo {
    render() {
        return null;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_arrow_iife_class_with_extends() {
    let input = r#"
var Foo = ((e) => {
    function t() {}
    ((e, t) => {
        e.prototype = Object.create(t && t.prototype, {
            constructor: { value: e, enumerable: false, writable: true, configurable: true }
        });
        t && (Object.setPrototypeOf ? Object.setPrototypeOf(e, t) : e.__proto__ = t);
    })(t, e);
    t.prototype.render = function() {
        return null;
    };
    return t;
})(Parent);
"#;
    let expected = r#"
class Foo extends Parent {
    render() {
        return null;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_arrow_iife_class_with_inherits_typecheck() {
    // Full Babel pattern with typeof check in inherits IIFE
    let input = r#"
var Foo = ((e) => {
    function t() {}
    ((e, t) => {
        if (typeof t != "function" && t !== null) {
            throw new TypeError("Super expression must either be null or a function");
        }
        e.prototype = Object.create(t && t.prototype, {
            constructor: { value: e, enumerable: false, writable: true, configurable: true }
        });
        t && (Object.setPrototypeOf ? Object.setPrototypeOf(e, t) : e.__proto__ = t);
    })(t, e);
    t.prototype.hello = function() {
        return "world";
    };
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
    hello() {
        return "world";
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_super_call_rewritten_in_constructor() {
    // e.call(this, args) should become super(args) in the constructor
    let input = r#"
var Foo = ((e) => {
    function t(x, y) {
        e.call(this, x, y);
        this.z = 1;
    }
    ((e, t) => {
        e.prototype = Object.create(t && t.prototype, {
            constructor: { value: e, enumerable: false, writable: true, configurable: true }
        });
    })(t, e);
    return t;
})(Parent);
"#;
    let expected = r#"
class Foo extends Parent {
    constructor(x, y){
        super(x, y);
        this.z = 1;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// super() || this simplification
// ============================================================

#[test]
fn test_super_or_this_simplified() {
    // `o = super(n, r) || this` → `o = super(n, r)` → cleanup aliases
    let input = r#"
var Foo = (function(e) {
    function t(n, r) {
        var o;
        o = e.call(this, n, r) || this;
        o.x = 1;
        return o;
    }
    t.prototype = Object.create(e && e.prototype);
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
    constructor(n, r){
        super(n, r);
        this.x = 1;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_super_or_this_direct_return() {
    // `return e.call(this) || this` → `return super()` → strip return
    let input = r#"
var Foo = (function(e) {
    function t() {
        return e.call(this) || this;
    }
    t.prototype = Object.create(e && e.prototype);
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_super_alias_replaced_with_this() {
    // n = r = super(...) → super(...), then r.x → this.x, return n removed
    let input = r#"
var Foo = ((e) => {
    function t() {
        var n;
        var r;
        n = r = e.call(this);
        r.state = { x: 1 };
        return n;
    }
    ((e, t) => {
        e.prototype = Object.create(t && t.prototype, {
            constructor: { value: e, enumerable: false, writable: true, configurable: true }
        });
    })(t, e);
    return t;
})(Parent);
"#;
    let expected = r#"
class Foo extends Parent {
    constructor(){
        super();
        this.state = {
            x: 1
        };
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_single_super_alias_replaced_with_this() {
    // r = super(...) → super(...), then r.x → this.x
    let input = r#"
var Foo = (function(e) {
    function t(a) {
        var r = e.call(this, a);
        r.name = a;
        return r;
    }
    t.prototype = Object.create(e && e.prototype);
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
    constructor(a){
        super(a);
        this.name = a;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_super_call_with_spread_rewritten() {
    let input = r#"
var Foo = (function(e) {
    function t() {
        e.call(this, a, b);
    }
    t.prototype = Object.create(e && e.prototype);
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
    constructor(){
        super(a, b);
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// super.apply(this, arguments) → super(...arguments)
// ============================================================

#[test]
fn test_super_apply_rewritten() {
    let input = r#"
var Foo = (function(e) {
    function t() {
        e.apply(this, arguments);
    }
    t.prototype = Object.create(e && e.prototype);
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Inline _possibleConstructorReturn IIFE unwrapping
// ============================================================

#[test]
fn test_inline_pcr_iife_with_apply() {
    // The pattern from module-24 classes z, Q, oe:
    // function t() { return PCR_IIFE(this, e.apply(this, arguments)); }
    let input = r#"
var Foo = (function(e) {
    function t() {
        return function(e, t) {
            if (!e) throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
            return !t || "object" != typeof t && "function" != typeof t ? e : t;
        }(this, e.apply(this, arguments));
    }
    t.prototype = Object.create(e && e.prototype);
    t.prototype.constructor = t;
    t.prototype.render = function render() { return null; }
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
    render() { return null; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_inline_pcr_arrow_iife_with_apply() {
    // Arrow form of inline PCR IIFE (as seen in decompiled output)
    let input = r#"
var Foo = ((e) => {
    function t() {
        return ((e, t) => {
            if (!e) throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
            if (!t || typeof t != "object" && typeof t != "function") return e;
            return t;
        })(this, e.apply(this, arguments));
    }
    ((e, t) => {
        e.prototype = Object.create(t && t.prototype, {
            constructor: { value: e, enumerable: false, writable: true, configurable: true }
        });
        t && (Object.setPrototypeOf ? Object.setPrototypeOf(e, t) : e.__proto__ = t);
    })(t, e);
    t.prototype.enable = function(e) { this.x = e; }
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
    enable(e) { this.x = e; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// Sequence-expression return: `return t.method = fn, ..., ClassName;`
// ============================================================

#[test]
fn test_seq_return_proto_alias_methods() {
    // Minified Babel loose: methods in comma expression return
    let input = r#"
var Foo = (function() {
    function e(a, b) { this.a = a; this.b = b; }
    var t = e.prototype;
    return t.getA = function getA() { return this.a; }, t.getB = function getB() { return this.b; }, e;
}());
"#;
    let expected = r#"
class Foo {
    constructor(a, b) { this.a = a; this.b = b; }
    getA() { return this.a; }
    getB() { return this.b; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_seq_return_with_extends() {
    // Minified Babel loose with inheritance: o(child, parent) + comma-expr return
    // Note: `|| this` fallback is a separate cleanup concern (not handled by UnEs6Class alone)
    let input = r#"
function o(e, t) {
    e.prototype = Object.create(t.prototype);
    e.prototype.constructor = e;
}
var Foo = (function(t) {
    o(a, t);
    var r = a.prototype;
    function a(n, r) {
        var o;
        o = t.call(this, n, r) || this;
        o.x = 1;
        return o;
    }
    return r.getX = function() { return this.x; }, r.render = function() { return null; }, a;
})(Parent);
"#;
    // `super(n, r) || this` is simplified to `super(n, r)`, then alias cleanup converts
    // `o = super(...)` → `super(); this.x = 1`
    let expected = r#"
function o(e, t) {
    e.prototype = Object.create(t.prototype);
    e.prototype.constructor = e;
}
class Foo extends Parent {
    constructor(n, r){
        super(n, r);
        this.x = 1;
    }
    getX() { return this.x; }
    render() { return null; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_seq_return_with_extends_direct_super() {
    // Same pattern but without the `|| this` fallback — alias should be cleaned up
    let input = r#"
function o(e, t) {
    e.prototype = Object.create(t.prototype);
    e.prototype.constructor = e;
}
var Foo = (function(t) {
    o(a, t);
    var r = a.prototype;
    function a(n, r) {
        var o;
        o = t.call(this, n, r);
        o.x = 1;
        return o;
    }
    return r.getX = function() { return this.x; }, r.render = function() { return null; }, a;
})(Parent);
"#;
    let expected = r#"
function o(e, t) {
    e.prototype = Object.create(t.prototype);
    e.prototype.constructor = e;
}
class Foo extends Parent {
    constructor(n, r){
        super(n, r);
        this.x = 1;
    }
    getX() { return this.x; }
    render() { return null; }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_seq_return_no_methods() {
    // Edge: just `return e;` (no comma expression) — already handled, verifying no regression
    let input = r#"
var Foo = (function() {
    function e() {}
    e.prototype.go = function go() {}
    return e;
}());
"#;
    let expected = r#"
class Foo {
    go() {}
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_inherits_helper_in_outer_scope() {
    // Module-23 pattern: inherits helper at top level, class IIFE inside a function body.
    // The inherits helper `o` is detected at module level and available in nested scopes.
    let input = r#"
function o(e, t) {
    e.prototype = Object.create(t.prototype);
    e.prototype.constructor = e;
}
function createProvider() {
    var r = (function(t) {
        o(a, t);
        var r = a.prototype;
        function a(n) { t.call(this, n); }
        r.render = function() { return null; };
        return a;
    })(Component);
    return r;
}
"#;
    let expected = r#"
function o(e, t) {
    e.prototype = Object.create(t.prototype);
    e.prototype.constructor = e;
}
function createProvider() {
    class r extends Component {
        constructor(n) { super(n); }
        render() { return null; }
    }
    return r;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn test_inline_pcr_with_comma_and_class_call_check() {
    // Full Babel pattern: classCallCheck, possibleConstructorReturn in sequence expr
    let input = r#"
var Foo = (function(e) {
    function t() {
        return function(e, t) {
            if (!(e instanceof t)) throw new TypeError("Cannot call a class as a function");
        }(this, t), function(e, t) {
            if (!e) throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
            return !t || "object" != typeof t && "function" != typeof t ? e : t;
        }(this, e.apply(this, arguments));
    }
    t.prototype = Object.create(e && e.prototype);
    t.prototype.constructor = t;
    return t;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ============================================================
// _createClass body-shape detection (minified helper name)
// ============================================================

use common::render;

#[test]
fn create_class_minified_helper_with_inline_inherits() {
    // Babel output where _createClass is minified to `r` and _inherits is inlined.
    // The outer IIFE has a param `e` but is called with 0 args.
    let input = r#"
var r = function() {
    function e(e, t) {
        for (var n = 0; n < t.length; n++) {
            var r = t[n];
            r.enumerable = r.enumerable || false;
            r.configurable = true;
            "value" in r && (r.writable = true);
            Object.defineProperty(e, r.key, r);
        }
    }
    return function(t, n, r) {
        return n && e(t.prototype, n), r && e(t, r), t;
    };
}();
var Foo = function(e) {
    function t() {
        return function(e, t) {
            if (!e) throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
            return !t || "object" != typeof t && "function" != typeof t ? e : t;
        }(this, (t.__proto__ || Object.getPrototypeOf(t)).apply(this, arguments));
    }
    return function(e, t) {
        if ("function" != typeof t && null !== t) throw new TypeError("Super expression must either be null or a function, not " + typeof t);
        e.prototype = Object.create(t && t.prototype, { constructor: { value: e, enumerable: false, writable: true, configurable: true } });
        t && (Object.setPrototypeOf ? Object.setPrototypeOf(e, t) : e.__proto__ = t);
    }(t, Bar), r(t, [
        { key: "render", value: function() { return 42; } }
    ]), t;
}();
"#;
    let result = render(input);
    assert!(
        result.contains("class Foo extends Bar"),
        "should produce class Foo extends Bar, got:\n{}",
        result
    );
    assert!(
        result.contains("render()"),
        "should have render method, got:\n{}",
        result
    );
    assert!(
        !result.contains("Object.defineProperty(e, r.key"),
        "createClass helper should be removed, got:\n{}",
        result
    );
}

#[test]
fn create_class_minified_no_super() {
    let input = r#"
var r = function() {
    function e(e, t) {
        for (var n = 0; n < t.length; n++) {
            var r = t[n];
            r.enumerable = r.enumerable || false;
            r.configurable = true;
            "value" in r && (r.writable = true);
            Object.defineProperty(e, r.key, r);
        }
    }
    return function(t, n, r) {
        return n && e(t.prototype, n), r && e(t, r), t;
    };
}();
var Foo = function() {
    function t(name) { this.name = name; }
    r(t, [
        { key: "greet", value: function() { return "hello " + this.name; } }
    ]);
    return t;
}();
"#;
    let result = render(input);
    assert!(
        result.contains("class Foo"),
        "should produce class Foo, got:\n{}",
        result
    );
    assert!(
        result.contains("greet()"),
        "should have greet method, got:\n{}",
        result
    );
    assert!(
        !result.contains("Object.defineProperty"),
        "createClass helper should be removed, got:\n{}",
        result
    );
}

#[test]
fn create_class_with_static_methods() {
    let input = r#"
var _createClass = function() {
    function e(e, t) {
        for (var n = 0; n < t.length; n++) {
            var r = t[n];
            r.enumerable = r.enumerable || false;
            r.configurable = true;
            "value" in r && (r.writable = true);
            Object.defineProperty(e, r.key, r);
        }
    }
    return function(t, n, r) {
        return n && e(t.prototype, n), r && e(t, r), t;
    };
}();
var Foo = function() {
    function t() {}
    _createClass(t, [
        { key: "instance", value: function() { return 1; } }
    ], [
        { key: "staticMethod", value: function() { return 2; } }
    ]);
    return t;
}();
"#;
    let result = render(input);
    assert!(
        result.contains("class Foo"),
        "should produce class Foo, got:\n{}",
        result
    );
    assert!(
        result.contains("instance()"),
        "should have instance method, got:\n{}",
        result
    );
    assert!(
        result.contains("static staticMethod()"),
        "should have static method, got:\n{}",
        result
    );
}
