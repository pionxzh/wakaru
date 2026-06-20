use super::matchers::{
    detect_helper_from_arrow, is_class_call_check_fn, is_object_without_properties_fn,
};
use super::*;
use swc_core::common::{sync::Lrc, FileName, Globals, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{
    CallExpr, Callee, Decl, Function, ImportSpecifier, ModuleDecl, ModuleItem, Pat, Stmt,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

fn parse_module(code: &str) -> Module {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(Lrc::new(FileName::Anon), code.to_string());
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser.parse_module().expect("failed to parse")
}

fn parse_first_function(code: &str) -> Function {
    let module = parse_module(code);
    for item in &module.body {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item {
            return *fn_decl.function.clone();
        }
    }
    panic!("no function declaration found in source");
}

fn module_has_function(module: &Module, name: &str) -> bool {
    module.body.iter().any(|item| {
        matches!(
            item,
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
                if fn_decl.ident.sym.as_ref() == name
        )
    })
}

fn module_has_var(module: &Module, name: &str) -> bool {
    module.body.iter().any(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            return false;
        };
        var.decls.iter().any(
            |decl| matches!(&decl.name, Pat::Ident(binding) if binding.id.sym.as_ref() == name),
        )
    })
}

fn module_has_import_local(module: &Module, name: &str) -> bool {
    module.body.iter().any(|item| {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            return false;
        };
        import.specifiers.iter().any(|specifier| match specifier {
            ImportSpecifier::Default(default) => default.local.sym.as_ref() == name,
            ImportSpecifier::Named(named) => named.local.sym.as_ref() == name,
            ImportSpecifier::Namespace(namespace) => namespace.local.sym.as_ref() == name,
        })
    })
}

#[test]
fn local_helper_context_collects_ts_helpers() {
    GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                import { __spreadArray as importedSpread } from "tslib";
                import * as tslibNs from "tslib";
                import { __awaiter as importedAwaiter } from "tslib";
                var aliasedAwaiter = (this && this.__awaiter) || function(thisArg, _arguments, P, generator) {
                    return new (P || (P = Promise))(function(resolve) {
                        resolve(generator.apply(thisArg, _arguments || []).next());
                    });
                };
                var aliasedGenerator = (this && this.__generator) || function(thisArg, body) {
                    return body.call(thisArg, { label: 0, sent: function() {}, trys: [], ops: [] });
                };
                function e(thisArg, body) {
                    var state = { label: 0, sent: function() {}, trys: [], ops: [] };
                    return body.call(thisArg, state);
                }
                function realStateMachine(user, options) {
                    var state = { label: 0, trys: [], ops: [] };
                    return options(state);
                }
                var inlineSpread = (this && this.__spreadArray) || function(to, from, pack) {
                    return to.concat(from);
                };
                var tslib_1 = require("tslib");
                var requiredSpread = require("tslib").__spreadArray;
                var requiredAwaiter = require("tslib").__awaiter;
                var namespaceSpread = tslib_1.__spreadArray;
                var namespaceAwaiter = tslib_1.__awaiter;
                var notSpread = customSpreadArray;
                var fakeAssign = (this && this.__assign) || customAssign;
                "#,
            );
            let helpers =
                LocalHelperContext::collect(&module).ts_helpers_of_kind(TsHelperKind::SpreadArray);

            assert_eq!(helpers.len(), 4);
            assert!(helpers
                .iter()
                .any(|(sym, _)| sym.as_ref() == "importedSpread"));
            assert!(helpers
                .iter()
                .any(|(sym, _)| sym.as_ref() == "inlineSpread"));
            assert!(helpers
                .iter()
                .any(|(sym, _)| sym.as_ref() == "requiredSpread"));
            assert!(!helpers.iter().any(|(sym, _)| sym.as_ref() == "notSpread"));

            let context = LocalHelperContext::collect(&module);
            let inline_helpers: HashMap<_, _> = context
                .ts_helpers
                .iter()
                .filter(|(_, helper)| helper.source == TsHelperSource::Inline)
                .map(|(key, helper)| (key.clone(), helper.kind))
                .collect();
            assert_eq!(
                inline_helpers
                    .get(&(Atom::from("aliasedAwaiter"), SyntaxContext::empty())),
                Some(&TsHelperKind::Awaiter)
            );
            assert_eq!(
                inline_helpers
                    .get(&(Atom::from("aliasedGenerator"), SyntaxContext::empty())),
                Some(&TsHelperKind::Generator)
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("e"), SyntaxContext::empty())),
                Some(&TsHelperKind::Generator)
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("realStateMachine"), SyntaxContext::empty())),
                None
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("importedAwaiter"), SyntaxContext::empty())),
                None
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("requiredAwaiter"), SyntaxContext::empty())),
                None
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("namespaceAwaiter"), SyntaxContext::empty())),
                None
            );

            let awaiter_helpers = context.ts_helpers_of_kind(TsHelperKind::Awaiter);
            assert_eq!(awaiter_helpers.len(), 4);

            let assign_helpers = context.ts_helpers_of_kind(TsHelperKind::Assign);
            assert!(
                !assign_helpers
                    .iter()
                    .any(|(sym, _)| sym.as_ref() == "fakeAssign"),
                "name-only inline helper candidates should not be collected"
            );

            assert!(
                context
                    .tslib_namespaces()
                    .contains(&(Atom::from("tslibNs"), SyntaxContext::empty()))
            );
            assert!(
                context
                    .tslib_namespaces()
                    .contains(&(Atom::from("tslib_1"), SyntaxContext::empty()))
            );
        });
}

#[test]
fn generated_function_with_label_property_is_not_ts_generator_helper() {
    GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                function L(effect, parentEffectId, label = "", extra) {
                    monitor.effectTriggered({
                        effectId: id,
                        parentEffectId,
                        label,
                        effect
                    });
                    use(effect, extra);
                }
                "#,
            );
            let context = LocalHelperContext::collect(&module);

            assert!(
                !context
                    .ts_helpers_of_kind(TsHelperKind::Generator)
                    .iter()
                    .any(|(sym, _)| sym.as_ref() == "L"),
                "ordinary generated-looking functions with a label property are not TS generator helpers"
            );
        });
}

#[test]
fn local_helper_context_collects_helper_dependencies() {
    GLOBALS.set(&Globals::new(), || {
        let module = parse_module(
            r#"
                function root(value) {
                    return dep(value);
                }
                function dep(value) {
                    return leaf(value);
                }
                function leaf(value) {
                    return value;
                }
                function unrelated(value) {
                    return dep(value);
                }
                "#,
        );
        let context = LocalHelperContext::collect(&module);
        let roots = HashMap::from([(
            (Atom::from("root"), SyntaxContext::empty()),
            TranspilerHelperKind::SlicedToArray,
        )]);

        let dependencies = context.helper_dependencies(&module, &roots);

        assert_eq!(
            dependencies.get(&(Atom::from("dep"), SyntaxContext::empty())),
            Some(&TranspilerHelperKind::HelperDependency)
        );
        assert_eq!(
            dependencies.get(&(Atom::from("leaf"), SyntaxContext::empty())),
            Some(&TranspilerHelperKind::HelperDependency)
        );
        assert!(!dependencies.contains_key(&(Atom::from("root"), SyntaxContext::empty())));
        assert!(!dependencies.contains_key(&(Atom::from("unrelated"), SyntaxContext::empty())));
    });
}

#[test]
fn removes_helpers_without_remaining_refs_only_when_unused() {
    GLOBALS.set(&Globals::new(), || {
        let mut unused = parse_module(
            r#"
                function helper(value) {
                    return value;
                }
                const value = 1;
                "#,
        );
        let helpers = HashMap::from([(
            (Atom::from("helper"), SyntaxContext::empty()),
            TranspilerHelperKind::ClassCallCheck,
        )]);

        remove_helpers_without_remaining_refs(&mut unused, helpers);

        assert!(!module_has_function(&unused, "helper"));

        let mut referenced = parse_module(
            r#"
                function helper(value) {
                    return value;
                }
                helper(1);
                "#,
        );
        let helpers = HashMap::from([(
            (Atom::from("helper"), SyntaxContext::empty()),
            TranspilerHelperKind::ClassCallCheck,
        )]);

        remove_helpers_without_remaining_refs(&mut referenced, helpers);

        assert!(module_has_function(&referenced, "helper"));
    });
}

#[test]
fn removes_helper_dependencies_with_consumed_root() {
    GLOBALS.set(&Globals::new(), || {
        let mut module = parse_module(
            r#"
                function root(value) {
                    return dep(value);
                }
                function dep(value) {
                    return value;
                }
                function unrelated(value) {
                    return value;
                }
                "#,
        );
        let context = LocalHelperContext::collect(&module);
        let roots = HashMap::from([(
            (Atom::from("root"), SyntaxContext::empty()),
            TranspilerHelperKind::SlicedToArray,
        )]);

        context.remove_helpers_with_dependencies(&mut module, roots);

        assert!(!module_has_function(&module, "root"));
        assert!(!module_has_function(&module, "dep"));
        assert!(module_has_function(&module, "unrelated"));
    });
}

#[test]
fn removes_unused_inline_ts_helpers_by_kind() {
    GLOBALS.set(&Globals::new(), || {
            let mut module = parse_module(
                r#"
                var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
                    return new (P || (P = Promise))(function(resolve) {
                        resolve(generator.apply(thisArg, _arguments || []).next());
                    });
                };
                var __generator = (this && this.__generator) || function (thisArg, body) {
                    return body.call(thisArg, { label: 0, sent: function() {}, trys: [], ops: [] });
                };
                import { __awaiter as importedAwaiter } from "tslib";
                "#,
            );
            let context = LocalHelperContext::collect(&module);

            context.remove_unused_inline_ts_helpers(
                &mut module,
                &[TsHelperKind::Awaiter, TsHelperKind::Generator],
            );

            assert!(!module_has_var(&module, "__awaiter"));
            assert!(!module_has_var(&module, "__generator"));
            assert!(module_has_import_local(&module, "importedAwaiter"));
        });
}

#[test]
fn removes_unused_ts_helper_bindings_by_kind() {
    GLOBALS.set(&Globals::new(), || {
        let mut module = parse_module(
            r#"
                import { __spreadArray } from "tslib";
                var spread = require("tslib").__spreadArray;
                var kept = require("tslib").__spreadArray;
                kept([], [], true);
                "#,
        );
        let context = LocalHelperContext::collect(&module);

        context.remove_unused_ts_helper_bindings(&mut module, TsHelperKind::SpreadArray);

        assert!(!module_has_import_local(&module, "__spreadArray"));
        assert!(!module_has_var(&module, "spread"));
        assert!(module_has_var(&module, "kept"));
    });
}

#[test]
fn local_helper_context_records_direct_tslib_require_member_calls() {
    GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                var a = require("tslib").__importDefault(require("a"));
                var b = require("tslib").__importStar(require("b"));
                var c = require("tslib").__read(values, 2);
                var d = require("not-tslib").__read(values, 2);
                "#,
            );
            let context = LocalHelperContext::collect(&module);

            assert!(
                context.has_tslib_require_member_call(TranspilerHelperKind::InteropRequireDefault)
            );
            assert!(
                context.has_tslib_require_member_call(TranspilerHelperKind::InteropRequireWildcard)
            );
            assert!(context.has_tslib_require_member_call(TranspilerHelperKind::SlicedToArray));
            assert!(!context.has_tslib_require_member_call(TranspilerHelperKind::ObjectSpread));
        });
}

#[test]
fn local_helper_context_matches_helper_callees() {
    GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                import * as tslibNs from "tslib";
                var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
                var local = _interopRequireDefault(require("local"));
                var namespaced = tslibNs.__importDefault(require("namespaced"));
                var direct = require("tslib").__importDefault(require("direct"));
                var unrelated = maybe.__importDefault(require("unrelated"));
                "#,
            );
            let context = LocalHelperContext::collect(&module);
            let callees: Vec<_> = module
                .body
                .iter()
                .filter_map(|item| {
                    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                        return None;
                    };
                    let decl = var.decls.first()?;
                    let Expr::Call(call) = decl.init.as_deref()? else {
                        return None;
                    };
                    let Callee::Expr(callee) = &call.callee else {
                        return None;
                    };
                    Some(callee.as_ref())
                })
                .collect();

            assert_eq!(
                context.helper_callee_kind(callees[1]),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
            assert_eq!(
                context.helper_callee_kind(callees[2]),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
            assert_eq!(
                context.helper_callee_kind(callees[3]),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
            assert_eq!(context.helper_callee_kind(callees[4]), None);
        });
}

#[test]
fn local_helper_context_collects_typeof_polyfill_helper() {
    GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                var _typeof = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
                    ? function(e) { return typeof e; }
                    : function(e) { return e && typeof Symbol == "function" ? "symbol" : typeof e; };
                var notTypeof = typeof window != "undefined" ? function(e) { return typeof e; } : function(e) { return e; };
                "#,
            );
            let helpers = LocalHelperContext::collect(&module).helpers_of_kind(TranspilerHelperKind::Typeof);

            assert_eq!(helpers.len(), 1);
            assert!(helpers.contains_key(&(Atom::from("_typeof"), SyntaxContext::empty())));
            assert!(!helpers.contains_key(&(Atom::from("notTypeof"), SyntaxContext::empty())));
        });
}

#[test]
fn local_helper_context_collects_tsc_private_field_helpers() {
    GLOBALS.set(&Globals::new(), || {
        let module = parse_module(
            r#"
                function __classPrivateFieldGet(receiver, state, kind, f) {
                    return state.get(receiver);
                }
                var __classPrivateFieldSet = function(receiver, state, value, kind, f) {
                    return state.set(receiver, value), value;
                };
                var A4 = function(receiver, state, value, kind) {
                    return state.set(receiver, value), value;
                };
                "#,
        );
        let context = LocalHelperContext::collect(&module);
        let getters = context.ts_helpers_of_kind(TsHelperKind::ClassPrivateFieldGet);
        let setters = context.ts_helpers_of_kind(TsHelperKind::ClassPrivateFieldSet);

        assert!(getters.contains(&(Atom::from("__classPrivateFieldGet"), SyntaxContext::empty())));
        assert!(setters.contains(&(Atom::from("__classPrivateFieldSet"), SyntaxContext::empty())));
        assert!(!setters.contains(&(Atom::from("A4"), SyntaxContext::empty())));
    });
}

#[test]
fn inline_legacy_spread_arrays_expression_matches_kind() {
    GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                var out = (this && this.__spreadArrays || function () {
                    for (var s = 0, i = 0, il = arguments.length; i < il; i++) s += arguments[i].length;
                    for (var r = Array(s), k = 0, i = 0; i < il; i++)
                        for (var a = arguments[i], j = 0, jl = a.length; j < jl; j++, k++)
                            r[k] = a[j];
                    return r;
                })([head], items, [tail]);
                "#,
            );
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &module.body[0] else {
                panic!("expected var decl");
            };
            let Expr::Call(call) = var.decls[0].init.as_deref().expect("init") else {
                panic!("expected call");
            };
            let Callee::Expr(callee) = &call.callee else {
                panic!("expected expr callee");
            };
            assert!(ts_expr_matches_helper_kind(
                callee,
                TsHelperKind::SpreadArrays
            ));
        });
}

#[test]
fn class_call_check_canonical() {
    GLOBALS.set(&Globals::new(), || {
        let f = parse_first_function(
            r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        throw new TypeError("Cannot call a class as a function");
                    }
                }"#,
        );
        assert!(is_class_call_check_fn(&f));
    });
}

#[test]
fn class_call_check_no_block_wrapping() {
    GLOBALS.set(&Globals::new(), || {
        let f = parse_first_function(
            r#"function _c(a, b) {
                    if (!(a instanceof b))
                        throw new TypeError("Cannot call a class as a function");
                }"#,
        );
        assert!(is_class_call_check_fn(&f));
    });
}

#[test]
fn class_call_check_with_parens() {
    GLOBALS.set(&Globals::new(), || {
        let f = parse_first_function(
            r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        throw new TypeError("Cannot call a class as a function");
                    }
                }"#,
        );
        assert!(is_class_call_check_fn(&f));
    });
}

#[test]
fn class_call_check_rejects_wrong_param_count() {
    GLOBALS.set(&Globals::new(), || {
        let f = parse_first_function(
            r#"function _c(a) {
                    if (!(a instanceof Foo)) {
                        throw new TypeError("nope");
                    }
                }"#,
        );
        assert!(!is_class_call_check_fn(&f));
    });
}

#[test]
fn class_call_check_rejects_swapped_operands() {
    GLOBALS.set(&Globals::new(), || {
        let f = parse_first_function(
            r#"function _c(a, b) {
                    if (!(b instanceof a)) {
                        throw new TypeError("nope");
                    }
                }"#,
        );
        assert!(!is_class_call_check_fn(&f));
    });
}

#[test]
fn class_call_check_rejects_non_instanceof() {
    GLOBALS.set(&Globals::new(), || {
        let f = parse_first_function(
            r#"function _c(a, b) {
                    if (!(a === b)) {
                        throw new TypeError("nope");
                    }
                }"#,
        );
        assert!(!is_class_call_check_fn(&f));
    });
}

#[test]
fn class_call_check_rejects_no_throw() {
    GLOBALS.set(&Globals::new(), || {
        let f = parse_first_function(
            r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        console.log("bad");
                    }
                }"#,
        );
        assert!(!is_class_call_check_fn(&f));
    });
}

#[test]
fn class_call_check_rejects_multiple_stmts() {
    GLOBALS.set(&Globals::new(), || {
        let f = parse_first_function(
            r#"function _c(a, b) {
                    var x = 1;
                    if (!(a instanceof b)) {
                        throw new TypeError("nope");
                    }
                }"#,
        );
        assert!(!is_class_call_check_fn(&f));
    });
}

#[test]
fn object_without_properties_spec_wrapper() {
    GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _objectWithoutProperties(e, t) {
                    if (null == e) return {};
                    var o, r, i = _objectWithoutPropertiesLoose(e, t);
                    if (Object.getOwnPropertySymbols) {
                        var n = Object.getOwnPropertySymbols(e);
                        for (r = 0; r < n.length; r++)
                            o = n[r], -1 === t.indexOf(o) && {}.propertyIsEnumerable.call(e, o) && (i[o] = e[o]);
                    }
                    return i;
                }"#,
            );
            assert!(is_object_without_properties_fn(&f));
        });
}

// -----------------------------------------------------------------------
// Inline (expression-site) helper detection
//
// These exercise `classify_inline_helper_call` directly so the shared
// body-shape recognition is unit-tested independent of the rules that
// consume it. Each test wraps a helper body in an IIFE: `(<callee>)(arg)`.
// -----------------------------------------------------------------------

/// Parse `var x = <call>;` and return the init call expression.
fn parse_first_call(code: &str) -> CallExpr {
    let module = parse_module(code);
    for item in &module.body {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
            if let Some(Expr::Call(call)) = var.decls.first().and_then(|d| d.init.as_deref()) {
                return call.clone();
            }
        }
    }
    panic!("no call expression found in source");
}

fn classify_first_call(code: &str) -> Option<TranspilerHelperKind> {
    let call = parse_first_call(code);
    classify_inline_helper_call(&call).map(|(kind, _)| kind)
}

/// Classify the callee of the first call expression directly, regardless of
/// argument count. Mirrors how multi-argument call sites (classCallCheck,
/// objectWithoutProperties) invoke the shared API.
fn classify_first_callee(code: &str) -> Option<TranspilerHelperKind> {
    let call = parse_first_call(code);
    let Callee::Expr(callee) = &call.callee else {
        panic!("expected expression callee");
    };
    classify_inline_callable(strip_parens(callee))
}

#[test]
fn inline_interop_default_ternary_arrow() {
    GLOBALS.set(&Globals::new(), || {
        assert_eq!(
            classify_first_call(r#"var x = ((e) => e && e.__esModule ? e : { default: e })(req);"#),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
    });
}

#[test]
fn inline_interop_default_ternary_return_block() {
    GLOBALS.set(&Globals::new(), || {
        assert_eq!(
            classify_first_call(
                r#"var x = (function(e) {
                        return e && e.__esModule ? e : { default: e };
                    })(req);"#
            ),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
    });
}

#[test]
fn inline_interop_default_if_return_arrow() {
    GLOBALS.set(&Globals::new(), || {
        assert_eq!(
            classify_first_call(
                r#"var x = ((e) => {
                        if (e && e.__esModule) { return e; }
                        return { default: e };
                    })(req);"#
            ),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
    });
}

#[test]
fn inline_interop_wildcard() {
    GLOBALS.set(&Globals::new(), || {
        assert_eq!(
            classify_first_call(
                r#"var x = ((e) => {
                        if (e && e.__esModule) { return e; }
                        var t = {};
                        if (e != null) {
                            for (var n in e) {
                                if (Object.prototype.hasOwnProperty.call(e, n)) { t[n] = e[n]; }
                            }
                        }
                        t.default = e;
                        return t;
                    })(req);"#
            ),
            Some(TranspilerHelperKind::InteropRequireWildcard)
        );
    });
}

#[test]
fn inline_class_call_check_arrow() {
    GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_callee(
                    r#"var x = ((e, t) => {
                        if (!(e instanceof t)) { throw new TypeError("Cannot call a class as a function"); }
                    })(this, Foo);"#
                ),
                Some(TranspilerHelperKind::ClassCallCheck)
            );
        });
}

#[test]
fn inline_class_call_check_fn_expr() {
    GLOBALS.set(&Globals::new(), || {
        assert_eq!(
            classify_first_callee(
                r#"var x = (function(e, t) {
                        if (!(e instanceof t)) { throw new TypeError("nope"); }
                    })(this, Foo);"#
            ),
            Some(TranspilerHelperKind::ClassCallCheck)
        );
    });
}

#[test]
fn inline_object_without_properties() {
    GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_callee(
                    r#"var x = ((e, t) => {
                        var n = {};
                        for (var r in e) {
                            t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
                        }
                        return n;
                    })(obj, ["a", "b"]);"#
                ),
                Some(TranspilerHelperKind::ObjectWithoutProperties)
            );
        });
}

#[test]
fn inline_helper_rejects_non_helper_iife() {
    GLOBALS.set(&Globals::new(), || {
        // __esModule guard with side effects + fallback is NOT an interop helper.
        assert_eq!(
            classify_first_call(
                r#"var x = ((e) => {
                        if (e && e.__esModule) { return e; }
                        sideEffect(e);
                        return fallback;
                    })(input);"#
            ),
            None
        );
        // Ordinary arithmetic IIFE.
        assert_eq!(
            classify_first_call(r#"var x = ((e) => { var a = e + 1; return a * 2; })(42);"#),
            None
        );
    });
}

#[test]
fn inline_helper_rejects_multiple_args() {
    GLOBALS.set(&Globals::new(), || {
        // classify_inline_helper_call requires exactly one argument; the
        // two-arg classCallCheck/OWP framing is validated by the call sites.
        let call = parse_first_call(
            r#"var x = ((e, t) => {
                    if (!(e instanceof t)) { throw new TypeError("nope"); }
                })(this, Foo);"#,
        );
        assert!(classify_inline_helper_call(&call).is_none());
        // ...but classifying the callable directly still recognizes the shape.
        if let Callee::Expr(callee) = &call.callee {
            assert_eq!(
                classify_inline_callable(strip_parens(callee)),
                Some(TranspilerHelperKind::ClassCallCheck)
            );
        } else {
            panic!("expected expression callee");
        }
    });
}

// -- declaration-site arrow detection (detect_helper_from_arrow) -----------

fn parse_first_arrow(code: &str) -> swc_core::ecma::ast::ArrowExpr {
    let module = parse_module(code);
    for item in &module.body {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
            if let Some(Expr::Arrow(arrow)) = var.decls.first().and_then(|d| d.init.as_deref()) {
                return arrow.clone();
            }
        }
    }
    panic!("no arrow expression found in source");
}

#[test]
fn arrow_decl_interop_default_ternary_expr() {
    GLOBALS.set(&Globals::new(), || {
        let arrow = parse_first_arrow(r#"var f = (e) => e && e.__esModule ? e : { default: e };"#);
        assert_eq!(
            detect_helper_from_arrow(&arrow, false),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
    });
}

#[test]
fn arrow_decl_interop_default_ternary_return_block() {
    GLOBALS.set(&Globals::new(), || {
        let arrow = parse_first_arrow(
            r#"var f = (e) => { return e && e.__esModule ? e : { default: e }; };"#,
        );
        assert_eq!(
            detect_helper_from_arrow(&arrow, false),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
    });
}

#[test]
fn arrow_decl_interop_default_if_return_block() {
    GLOBALS.set(&Globals::new(), || {
        let arrow = parse_first_arrow(
            r#"var f = (e) => { if (e && e.__esModule) return e; return { default: e }; };"#,
        );
        assert_eq!(
            detect_helper_from_arrow(&arrow, false),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
    });
}

#[test]
fn arrow_decl_object_without_properties() {
    GLOBALS.set(&Globals::new(), || {
            let arrow = parse_first_arrow(
                r#"var f = (e, t) => {
                    var n = {};
                    for (var r in e) {
                        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
                    }
                    return n;
                };"#,
            );
            assert_eq!(
                detect_helper_from_arrow(&arrow, false),
                Some(TranspilerHelperKind::ObjectWithoutProperties)
            );
        });
}

#[test]
fn arrow_decl_to_consumable_array_threads_has_sub_helpers() {
    GLOBALS.set(&Globals::new(), || {
            // Babel 7+ OR-chain dispatcher form is only a helper when the module
            // carries sub-helper signals — pins that has_sub_helpers is threaded
            // through the arrow path unchanged.
            let arrow = parse_first_arrow(
                r#"var f = (arr) => { return _arrayWithoutHoles(arr) || _iterableToArray(arr) || _nonIterableSpread(); };"#,
            );
            assert_eq!(
                detect_helper_from_arrow(&arrow, true),
                Some(TranspilerHelperKind::ToConsumableArray)
            );
            assert_eq!(detect_helper_from_arrow(&arrow, false), None);
        });
}

#[test]
fn arrow_decl_non_helper_is_none() {
    GLOBALS.set(&Globals::new(), || {
        let arrow = parse_first_arrow(r#"var f = (e) => { var a = e + 1; return a * 2; };"#);
        assert_eq!(detect_helper_from_arrow(&arrow, false), None);
    });
}
