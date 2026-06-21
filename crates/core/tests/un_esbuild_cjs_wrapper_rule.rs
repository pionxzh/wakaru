mod common;

use common::{assert_eq_normalized, render_pipeline};
use wakaru_core::{decompile, DecompileOptions, RewriteLevel};

fn apply(input: &str) -> String {
    render_pipeline(input)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code
}

#[test]
fn unwraps_inlined_single_module_commonjs_wrapper() {
    let input = r#"
const require_stdin = ((modules, cache) => function require_stdin() {
    if (!cache) {
        modules[Object.getOwnPropertyNames(modules)[0]]((cache = { exports: {} }).exports, cache);
    }
    return cache.exports;
})({
    "<stdin>"(exports) {
        use(async (app_id) => await fetch_user(app_id));
    }
});
export default require_stdin();
"#;

    let expected = r#"
use(async (app_id) => await fetch_user(app_id));
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn unwraps_nested_async_arrow_body() {
    let input = r#"
const require_stdin = ((modules, cache) => function require_stdin() {
    if (!cache) {
        modules[Object.getOwnPropertyNames(modules)[0]]((cache = { exports: {} }).exports, cache);
    }
    return cache.exports;
})({
    "<stdin>"(exports) {
        use(async (source) => {
            return (await load_steps(source)).map(async (step) => await step.run(source));
        });
    }
});
export default require_stdin();
"#;

    let expected = r#"
use(async (source) => (await load_steps(source)).map(async (step) => await step.run(source)));
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_wrapper_when_exports_is_used() {
    let input = r#"
const require_value = ((modules, cache) => function require_value() {
    if (!cache) {
        modules[Object.getOwnPropertyNames(modules)[0]]((cache = { exports: {} }).exports, cache);
    }
    return cache.exports;
})({
    "value.js"(exports) {
        exports.value = compute();
    }
});
export default require_value();
"#;

    let output = apply(input);
    assert!(
        output.contains("export default require_value()"),
        "wrapper must stay when the CommonJS export object is used:\n{output}"
    );
}

#[test]
fn keeps_wrapper_when_default_export_passes_arguments() {
    let input = r#"
const require_stdin = ((modules, cache) => function require_stdin() {
    if (!cache) {
        modules[Object.getOwnPropertyNames(modules)[0]]((cache = { exports: {} }).exports, cache);
    }
    return cache.exports;
})({
    "<stdin>"(exports) {
        use(async (app_id) => await fetch_user(app_id));
    }
});
export default require_stdin(force);
"#;

    let output = apply(input);
    assert!(
        output.contains("export default require_stdin(force)"),
        "wrapper must stay when the default export call is not the plain module initializer:\n{output}"
    );
}

#[test]
fn minimal_preserves_inlined_single_module_commonjs_wrapper() {
    let input = r#"
const require_stdin = ((modules, cache) => function require_stdin() {
    if (!cache) {
        modules[Object.getOwnPropertyNames(modules)[0]]((cache = { exports: {} }).exports, cache);
    }
    return cache.exports;
})({
    "<stdin>"(exports) {
        use(async (app_id) => await fetch_user(app_id));
    }
});
export default require_stdin();
"#;

    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert!(
        output.contains("export default require_stdin()"),
        "minimal mode should preserve the wrapper:\n{output}"
    );
}
