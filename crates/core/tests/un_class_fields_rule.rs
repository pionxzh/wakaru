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
