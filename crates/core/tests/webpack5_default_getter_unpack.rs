//! webpack 5 reads the default import of a module it could not prove ESM
//! through `require.n(x)` and each named import as a member of `x`. For a
//! provider marked `__esModule`, `.n` returns `exports.default`, so the
//! getter reads must not become the binding itself.

use wakaru_core::driver::test_support::unpack;
use wakaru_core::{validate_output_modules, DecompileOptions};

fn expect_unpack(source: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
}

fn module<'a>(modules: &'a [(String, String)], name: &str) -> &'a str {
    &modules
        .iter()
        .find(|(file, _)| file == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} should exist, got {:?}",
                modules.iter().map(|(file, _)| file).collect::<Vec<_>>()
            )
        })
        .1
}

fn bundle(modules: &str) -> String {
    format!(
        r#"(() => {{
    "use strict";
    var __webpack_modules__ = ({{
{modules}
    }});
    const __webpack_module_cache__ = {{}};
    function __webpack_require__(moduleId) {{
        const cachedModule = __webpack_module_cache__[moduleId];
        if (cachedModule !== undefined) {{
            return cachedModule.exports;
        }}
        const module = __webpack_module_cache__[moduleId] = {{
            exports: {{}}
        }};
        __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
        return module.exports;
    }}
    __webpack_require__.n = (module) => {{
        const getter = module && module.__esModule ?
            () => (module['default']) :
            () => (module);
        __webpack_require__.d(getter, {{ a: getter }});
        return getter;
    }};
    __webpack_require__.d = (exports, definition) => {{
        for(var key in definition) {{
            if(__webpack_require__.o(definition, key) && !__webpack_require__.o(exports, key)) {{
                Object.defineProperty(exports, key, {{ enumerable: true, get: definition[key] }});
            }}
        }}
    }};
    __webpack_require__.o = (obj, prop) => (Object.prototype.hasOwnProperty.call(obj, prop));
    let __webpack_exports__ = {{}};
    var _main__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__(10);
    globalThis.sink = Object.freeze({{
        get run() {{ return _main__WEBPACK_IMPORTED_MODULE_0__.e; }},
    }});
}})();
"#
    )
}

/// A provider built from ESM by swc: marked, with a default and named
/// exports written through `_export` getters.
const MARKED_PROVIDER: &str = r#"
        20(__unused_webpack_module, exports) {
            Object.defineProperty(exports, "__esModule", {
                value: true
            });
            function _export(target, all) {
                for(var name in all)Object.defineProperty(target, name, {
                    enumerable: true,
                    get: Object.getOwnPropertyDescriptor(all, name).get
                });
            }
            _export(exports, {
                get tick () {
                    return tick;
                },
                get total () {
                    return total;
                },
                get default () {
                    return Gadget;
                }
            });
            let total = 0;
            function tick() {
                total += 1;
            }
            class Gadget {
                constructor(name){
                    this.name = name;
                }
            }
        }
"#;

fn consumer(getter_reads: &str) -> String {
    format!(
        r#"
        10(__unused_webpack_module, __webpack_exports__, __webpack_require__) {{
            __webpack_require__.d(__webpack_exports__, {{
                e: () => (run)
            }});
            var _gadget__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__(20);
            var _gadget__WEBPACK_IMPORTED_MODULE_0___default = __webpack_require__.n(_gadget__WEBPACK_IMPORTED_MODULE_0__);
            function run() {{
                const before = _gadget__WEBPACK_IMPORTED_MODULE_0__.total;
                (0, _gadget__WEBPACK_IMPORTED_MODULE_0__.tick)();
                return [{getter_reads}, before, _gadget__WEBPACK_IMPORTED_MODULE_0__.total];
            }}
        }},
"#
    )
}

#[test]
fn getter_call_of_a_marked_provider_reads_its_default() {
    let source = bundle(&format!(
        "{}{}",
        consumer(r#"new (_gadget__WEBPACK_IMPORTED_MODULE_0___default())("one")"#),
        MARKED_PROVIDER
    ));
    let modules = expect_unpack(&source);
    let main = module(&modules, "module-10.js");
    assert!(
        main.contains(r#"import gadget, { tick, total } from "./module-20.js";"#)
            || main.contains(r#"{ tick, total } from "./module-20.js";"#),
        "the default and the named exports should be separate imports:\n{main}"
    );
    assert!(
        !main.contains(".tick") && !main.contains(".total"),
        "named exports should not be read as members of the default:\n{main}"
    );
    assert!(
        validate_output_modules(&modules).is_empty(),
        "unexpected graph findings: {:#?}",
        validate_output_modules(&modules)
    );
}

#[test]
fn dot_a_getter_read_of_a_marked_provider_reads_its_default() {
    let source = bundle(&format!(
        "{}{}",
        consumer(r#"new _gadget__WEBPACK_IMPORTED_MODULE_0___default.a("one")"#),
        MARKED_PROVIDER
    ));
    let modules = expect_unpack(&source);
    let main = module(&modules, "module-10.js");
    assert!(
        !main.contains(".tick") && !main.contains(".total"),
        "named exports should not be read as members of the default:\n{main}"
    );
    assert!(
        validate_output_modules(&modules).is_empty(),
        "unexpected graph findings: {:#?}",
        validate_output_modules(&modules)
    );
}

#[test]
fn getter_call_of_an_unmarked_provider_reads_the_whole_exports() {
    // Without `__esModule`, `.n` returns `module.exports` itself, so the
    // getter read is the required value and `.total` is a member of it.
    let provider = r#"
        20(module) {
            function Gadget(name) {
                this.name = name;
            }
            Gadget.total = 0;
            Gadget.tick = function() {
                Gadget.total += 1;
            };
            module.exports = Gadget;
        }
"#;
    let source = bundle(&format!(
        "{}{}",
        consumer(r#"new (_gadget__WEBPACK_IMPORTED_MODULE_0___default())("one")"#),
        provider
    ));
    let modules = expect_unpack(&source);
    let main = module(&modules, "module-10.js");
    assert!(
        !main.contains(".default"),
        "the getter read is the whole value, not its default:\n{main}"
    );
    assert!(
        main.contains(".total"),
        "`total` stays a member of the required value:\n{main}"
    );
}

#[test]
fn unused_getter_does_not_keep_a_default_import() {
    // webpack declares the getter even when the source imported only names;
    // the provider has no default export, so a default import cannot link.
    let consumer = r#"
        10(__unused_webpack_module, __webpack_exports__, __webpack_require__) {
            __webpack_require__.d(__webpack_exports__, {
                e: () => (run)
            });
            var _gadget__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__(20);
            var _gadget__WEBPACK_IMPORTED_MODULE_0___default = __webpack_require__.n(_gadget__WEBPACK_IMPORTED_MODULE_0__);
            function run() {
                return [_gadget__WEBPACK_IMPORTED_MODULE_0__.total, (0, _gadget__WEBPACK_IMPORTED_MODULE_0__.tick)()];
            }
        },
"#;
    let provider = r#"
        20(__unused_webpack_module, exports) {
            Object.defineProperty(exports, "__esModule", {
                value: true
            });
            exports.tick = exports.total = void 0;
            exports.total = 0;
            function tick() {
                exports.total += 1;
            }
            exports.tick = tick;
        }
"#;
    let source = bundle(&format!("{consumer}{provider}"));
    let modules = expect_unpack(&source);
    let main = module(&modules, "module-10.js");
    assert!(
        main.contains(r#"import { tick, total } from "./module-20.js";"#),
        "only the named imports should remain:\n{main}"
    );
    assert!(
        validate_output_modules(&modules).is_empty(),
        "unexpected graph findings: {:#?}",
        validate_output_modules(&modules)
    );
}

#[test]
fn getter_call_of_a_marked_provider_that_stays_commonjs_reads_its_default() {
    // The consumer writes to the provider's exports, so the provider stays
    // CommonJS and the default import holds its `module.exports`: the getter
    // read is that object's `default`.
    let consumer = r#"
        10(__unused_webpack_module, __webpack_exports__, __webpack_require__) {
            __webpack_require__.d(__webpack_exports__, {
                e: () => (run)
            });
            var _gadget__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__(20);
            var _gadget__WEBPACK_IMPORTED_MODULE_0___default = __webpack_require__.n(_gadget__WEBPACK_IMPORTED_MODULE_0__);
            function run() {
                _gadget__WEBPACK_IMPORTED_MODULE_0__.extra = 1;
                return [new (_gadget__WEBPACK_IMPORTED_MODULE_0___default())("one"), _gadget__WEBPACK_IMPORTED_MODULE_0__.total];
            }
        },
"#;
    let provider = r#"
        20(__unused_webpack_module, exports) {
            Object.defineProperty(exports, "__esModule", {
                value: true
            });
            exports.default = exports.total = void 0;
            exports.total = 0;
            class Gadget {
                constructor(name){
                    this.name = name;
                }
            }
            exports.default = Gadget;
        }
"#;
    let source = bundle(&format!("{consumer}{provider}"));
    let modules = expect_unpack(&source);
    let main = module(&modules, "module-10.js");
    assert!(
        main.contains(".default(\"one\")"),
        "the getter read should be the default of the required value:\n{main}"
    );
    assert!(
        main.contains(".total"),
        "`total` stays a member of the required value:\n{main}"
    );
}
