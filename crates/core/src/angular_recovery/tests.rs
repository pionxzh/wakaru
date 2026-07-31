use super::*;

const PRODUCTION_COMPONENT: &str = r#"
import * as core from "@angular/core";

export class DemoCardComponent {
    title = "Example";
    disabled = false;
    select() {
        this.disabled = true;
    }

    static ɵfac = function DemoCardComponent_Factory(type) {
        return new (type || DemoCardComponent)();
    };

    static ɵcmp = core.ɵɵdefineComponent({
        type: DemoCardComponent,
        selectors: [["demo-card"]],
        decls: 5,
        vars: 2,
        consts: [
            ["class", "card"],
            ["type", "button", 3, "click", "disabled"]
        ],
        template: function DemoCardComponent_Template(renderFlags, context) {
            if (renderFlags & 1) {
                core.ɵɵelementStart(0, "article", 0)(1, "h2");
                core.ɵɵtext(2);
                core.ɵɵelementEnd();
                core.ɵɵelementStart(3, "button", 1);
                core.ɵɵlistener("click", function DemoCardComponent_button_click() {
                    return context.select();
                });
                core.ɵɵtext(4, "Select");
                core.ɵɵelementEnd()();
            }
            if (renderFlags & 2) {
                core.ɵɵadvance(2);
                core.ɵɵtextInterpolate(context.title);
                core.ɵɵadvance();
                core.ɵɵproperty("disabled", context.disabled);
            }
        },
        styles: [
            "[_nghost-%COMP%] { display: block; }\narticle[_ngcontent-%COMP%] { padding: 1rem; }"
        ]
    });
}
"#;

#[test]
fn recovers_production_component_as_inline_template_typescript() {
    assert!(!PRODUCTION_COMPONENT.contains("ɵsetClassMetadata"));
    assert!(!PRODUCTION_COMPONENT.contains("template: `"));
    assert!(!PRODUCTION_COMPONENT.contains("<article>"));

    let recovered =
        recover_angular_components_from_js(PRODUCTION_COMPONENT, AngularRecoveryOptions::default())
            .expect("production Ivy should parse");

    assert_eq!(recovered.len(), 1);
    let component = &recovered[0];
    assert_eq!(component.name, "DemoCardComponent");
    assert_eq!(component.selector, "demo-card");
    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert_eq!(
        component.source,
        r#"import { Component } from "@angular/core";

@Component({
  selector: "demo-card",
  template: `
    <article class="card">
      <h2>{{ title }}</h2>
      <button type="button" (click)="select()" [disabled]="disabled">Select</button>
    </article>
  `,
  styles: [
    `
      :host { display: block; }
      article { padding: 1rem; }
    `,
  ],
})
export class DemoCardComponent {
    title = "Example";
    disabled = false;
    select() {
        this.disabled = true;
    }
}"#
    );
}

#[test]
fn accepts_named_ivy_instruction_imports() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class BadgeComponent {
            static ɵcmp = define({
                type: BadgeComponent,
                selectors: [["demo-badge"]],
                decls: 1,
                vars: 0,
                template: function(rf) {
                    if (rf & 1) {
                        element(0, "hr");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("named imports should parse");
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].source.contains("<hr />"));
}

#[test]
fn restores_named_angular_class_api_imports() {
    let source = r#"
        import {
            computed as a,
            inject as b,
            input as c,
            model as d,
            output as e,
            signal as f,
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class ApiCardComponent {
            value = c("reader");
            requiredValue = c.required();
            count = f(0);
            label = a(() => this.value());
            service = b(Service);
            selection = d("");
            changed = e();

            static ɵcmp = define({
                type: ApiCardComponent,
                selectors: [["api-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "section");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Angular class API imports should parse");

    assert_eq!(recovered.len(), 1);
    let source = &recovered[0].source;
    assert!(source.contains(
        "import { Component, computed, inject, input, model, output, signal } from \"@angular/core\";"
    ));
    for expected in [
        "value = input(\"reader\")",
        "requiredValue = input.required()",
        "count = signal(0)",
        "label = computed(()=>this.value())",
        "service = inject(Service)",
        "selection = model(\"\")",
        "changed = output()",
    ] {
        assert!(source.contains(expected), "missing {expected:?}:\n{source}");
    }
}

#[test]
fn restores_named_signal_query_apis_from_compiled_metadata() {
    let source = r#"
        import {
            contentChild as contentOne,
            contentChildren as contentMany,
            viewChild as viewOne,
            viewChildren as viewMany,
            ɵɵcontentQuerySignal as registerContent,
            ɵɵdefineComponent as define,
            ɵɵelement as element,
            ɵɵqueryAdvance as advance,
            ɵɵviewQuerySignal as registerView,
        } from "@angular/core";

        const contentOptionalPredicate = ["contentOptional"];
        const contentRequiredPredicate = ["contentRequired"];
        const contentManyPredicate = ["contentMany"];
        const viewOptionalPredicate = ["viewOptional"];
        const viewRequiredPredicate = ["viewRequired"];
        const viewManyPredicate = ["viewMany"];
        const QueryToken = class {};

        class QueryApiComponent {
            viewOptional = viewOne("discarded");
            viewRequired = viewOne.required("discarded");
            viewMany = viewMany("discarded");
            viewToken = viewOne(QueryToken);
            contentOptional = contentOne("discarded");
            contentRequired = contentOne.required("discarded");
            contentMany = contentMany("discarded");

            static compiled = define({
                type: QueryApiComponent,
                selectors: [["query-api"]],
                contentQueries: function(renderFlags, context, directiveIndex) {
                    if (renderFlags & 1) {
                        registerContent(
                            directiveIndex,
                            context.contentOptional,
                            contentOptionalPredicate,
                            4
                        )(
                            directiveIndex,
                            context.contentRequired,
                            contentRequiredPredicate,
                            5,
                            ReadToken
                        )(
                            directiveIndex,
                            context.contentMany,
                            contentManyPredicate,
                            4
                        );
                    }
                    if (renderFlags & 2) {
                        advance(3);
                    }
                },
                viewQuery: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        registerView(
                            context.viewOptional,
                            viewOptionalPredicate,
                            5
                        )(
                            context.viewRequired,
                            viewRequiredPredicate,
                            5
                        )(
                            context.viewMany,
                            viewManyPredicate,
                            5,
                            ReadToken
                        )(
                            context.viewToken,
                            QueryToken,
                            5
                        );
                    }
                    if (renderFlags & 2) {
                        advance(4);
                    }
                },
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "section");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("compiled signal query metadata should parse");

    assert_eq!(recovered.len(), 1);
    let source = &recovered[0].source;
    assert!(
        source.contains(
            "import { Component, contentChild, contentChildren, viewChild, viewChildren } from \"@angular/core\";"
        ),
        "{source}"
    );
    for expected in [
        "viewOptional = viewChild(\"viewOptional\")",
        "viewRequired = viewChild.required(\"viewRequired\")",
        "viewMany = viewChildren(\"viewMany\", {",
        "viewToken = viewChild(QueryToken)",
        "read: ReadToken",
        "contentOptional = contentChild(\"contentOptional\", {",
        "descendants: false",
        "contentRequired = contentChild.required(\"contentRequired\", {",
        "contentMany = contentChildren(\"contentMany\")",
    ] {
        assert!(source.contains(expected), "missing {expected:?}:\n{source}");
    }
    assert!(!source.contains("\"discarded\""));
}

#[test]
fn uses_query_metadata_to_disambiguate_closure_identical_factories() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        function makeQuery(firstOnly, required) {
            let query = computed(function() {
                const result = refreshQuery(query, firstOnly);
                if (required && result === undefined) {
                    throw new RuntimeError(-951, false);
                }
                return firstOnly ? result.first : result;
            });
            return query;
        }
        function one() {
            return makeQuery(true, false);
        }
        one.required = function() {
            return makeQuery(true, true);
        };
        const sharedOne = one;
        function many() {
            return makeQuery(false, false);
        }

        function registerView(target, predicate, flags, read) {
            bindQuery(target, createViewQuery(predicate, flags, read));
            return registerView;
        }
        function registerContent(directiveIndex, target, predicate, flags, read) {
            bindQuery(
                target,
                createContentQuery(directiveIndex, predicate, flags, read)
            );
            return registerContent;
        }

        class ClosureQueryComponent {
            constructor() {
                this.viewOptional = sharedOne();
                this.viewRequired = sharedOne.required();
                this.viewMany = many();
                this.contentOptional = sharedOne();
                this.contentRequired = sharedOne.required();
                this.contentMany = many();
            }

            static compiled = define({
                type: ClosureQueryComponent,
                selectors: [["closure-query"]],
                contentQueries: function(renderFlags, context, directiveIndex) {
                    if (renderFlags & 1) {
                        registerContent(
                            directiveIndex,
                            context.contentOptional,
                            ["contentOptional"],
                            4
                        )(
                            directiveIndex,
                            context.contentRequired,
                            ["contentRequired"],
                            5
                        )(
                            directiveIndex,
                            context.contentMany,
                            ["contentMany"],
                            4
                        );
                    }
                },
                viewQuery: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        registerView(
                            context.viewOptional,
                            ["viewOptional"],
                            5
                        )(
                            context.viewRequired,
                            ["viewRequired"],
                            5
                        )(
                            context.viewMany,
                            ["viewMany"],
                            5
                        );
                    }
                },
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "section");
                    }
                },
            });
        }

        class QueryHelperWithoutMetadataComponent {
            constructor() {
                this.opaque = sharedOne();
            }

            static compiled = define({
                type: QueryHelperWithoutMetadataComponent,
                selectors: [["query-helper-without-metadata"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "aside");
                    }
                },
            });
        }

        class StaticQueryMetadataComponent {
            constructor() {
                this.opaque = sharedOne();
            }

            static compiled = define({
                type: StaticQueryMetadataComponent,
                selectors: [["static-query-metadata"]],
                viewQuery: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        registerView(context.opaque, ["opaque"], 7);
                    }
                },
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "aside");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure query factories should be correlated with metadata");
    let by_selector = recovered
        .iter()
        .map(|component| (component.selector.as_str(), component))
        .collect::<HashMap<_, _>>();

    let query = &by_selector["closure-query"].source;
    for expected in [
        "viewOptional = viewChild(\"viewOptional\")",
        "viewRequired = viewChild.required(\"viewRequired\")",
        "viewMany = viewChildren(\"viewMany\")",
        "contentOptional = contentChild(\"contentOptional\", {",
        "descendants: false",
        "contentRequired = contentChild.required(\"contentRequired\")",
        "contentMany = contentChildren(\"contentMany\")",
    ] {
        assert!(query.contains(expected), "missing {expected:?}:\n{query}");
    }

    let opaque = &by_selector["query-helper-without-metadata"].source;
    assert!(opaque.contains("opaque = sharedOne()"), "{opaque}");
    assert!(!opaque.contains("viewChild"), "{opaque}");
    assert!(!opaque.contains("contentChild"), "{opaque}");

    let static_query = &by_selector["static-query-metadata"].source;
    assert!(
        static_query.contains("opaque = sharedOne()"),
        "{static_query}"
    );
    assert!(!static_query.contains("viewChild"), "{static_query}");
}

#[test]
fn does_not_introduce_an_angular_api_import_that_would_be_shadowed() {
    let source = r#"
        import {
            input as a,
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class ApiCollisionComponent {
            read(input) {
                return a(input);
            }

            static ɵcmp = define({
                type: ApiCollisionComponent,
                selectors: [["api-collision"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "section");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("shadowed Angular API imports should parse");

    assert_eq!(recovered.len(), 1);
    let source = &recovered[0].source;
    assert!(!source.contains("Component, input"));
    assert!(source.contains("import { input as a } from \"@angular/core\";"));
    assert!(source.contains("return a(input)"));
}

#[test]
fn infers_closure_renamed_angular_class_api_families() {
    let source = r#"
        this.shared = this.shared || {};
        (function(runtime) {
            function makeComputed(computation, equal) {
                const node = Object.create(computedNode);
                node.computation = computation;
                if (equal !== undefined) {
                    node.equal = equal;
                }
                let read = () => {
                    updateProducer(node);
                    trackProducer(node);
                    if (node.value === errored) {
                        throw node.error;
                    }
                    return node.value;
                };
                read[nodeKey] = node;
                return read;
            }

            function makeSignal(initial, equal) {
                const node = Object.create(signalNode);
                node.value = initial;
                if (equal !== undefined) {
                    node.equal = equal;
                }
                const read = () => {
                    trackProducer(node);
                    return node.value;
                };
                read[nodeKey] = node;
                return [
                    read,
                    (value) => setSignal(node, value),
                    (update) => setSignal(node, update(node.value)),
                ];
            }
            function makeZeroSignal() {
                const node = Object.create(signalNode);
                node.value = 0;
                const read = () => {
                    trackProducer(node);
                    return node.value;
                };
                return read[nodeKey] = node, [
                    read,
                    (value) => setSignal(node, value),
                    (update) => setSignal(node, update(node.value)),
                ];
            }
            class OutputRef {
                subscribe(listener) {
                    if (this.destroyed) {
                        throw new RuntimeError(953, false);
                    }
                    this.listeners.push(listener);
                    return {
                        unsubscribe: () => removeListener(this.listeners, listener),
                    };
                }
                emit(value) {
                    if (this.destroyed) {
                        warnAboutDestroyedOutput(953);
                    }
                    notifyListeners(this.listeners, value);
                }
            }

            function injectFlags(options) {
                if (typeof options > "u" || typeof options === "number") {
                    return options;
                }
                return 0
                    | (options.optional && 8)
                    | (options.host && 1)
                    | (options.self && 2)
                    | (options.skipSelf && 4);
            }

            function injectImpl(token, flags) {
                return currentInjector(token, flags);
            }

            runtime.a = (initial, options) => {
                const [read, set, update] = makeSignal(initial, options?.equal);
                read.set = set;
                read.update = update;
                read.asReadonly = asReadonly.bind(read);
                return read;
            };
            runtime.a0 = () => {
                const [read, set, update] = makeZeroSignal();
                read.set = set;
                read.update = update;
                read.renamedReadonly = asReadonly.bind(read);
                return read;
            };
            const zeroSignalAlias = runtime.a0;
            runtime.b = (computation, options) =>
                makeComputed(computation, options?.equal);
            runtime.b2 = function(computation) {
                const node = Object.create(computedNode);
                node.computation = computation;
                computation = () => {
                    updateProducer(node);
                    trackProducer(node);
                    if (node.value === errored) {
                        throw node.error;
                    }
                    return node.value;
                };
                return computation[nodeKey] = node, computation;
            };
            runtime.c = (token, options) =>
                injectImpl(token, injectFlags(options));
            runtime.d = (initial, options) => {
                function read() {
                    trackProducer(node);
                    if (node.value === requiredUnset) {
                        throw new RuntimeError(-950, null);
                    }
                    return node.value;
                }
                const node = Object.create(inputNode);
                node.value = initial;
                node.transform = options?.transform;
                return read[nodeKey] = node, read;
            };
            runtime.e = (initial, options) => runtime.d(initial, options);
            runtime.f = (initial) => {
                function read() {
                    trackProducer(node);
                    if (node.value === requiredUnset) {
                        throw new RuntimeError(952, false);
                    }
                    return node.value;
                }
                const node = Object.create(inputNode);
                const emitter = createEmitter();
                node.value = initial;
                read.asReadonly = asReadonly.bind(read);
                read.set = (value) => setSignal(node, value);
                read.update = (update) => read.set(update(node.value));
                read.subscribe = emitter.subscribe.bind(emitter);
                return read[nodeKey] = node, read;
            };
            runtime.g = (initial) => runtime.f(initial);
            runtime.o = () => new OutputRef();
            const modelAlias = (
                runtime.g.required = () => runtime.f(requiredUnset),
                runtime.g
            );

            runtime.define = function(definition) {
                return definition;
            };
            runtime.element = function() {
                return runtime.element;
            };
            const publicRuntime = {
                "ɵɵdefineComponent": runtime.define,
                "ɵɵelement": runtime.element,
            };

            runtime.ApiCard = class {
                count = runtime.a(0);
                zeroCount = runtime.a0();
                aliasedZeroCount = zeroSignalAlias();
                label = runtime.b(() => this.count());
                specializedLabel = runtime.b2(() => this.count());
                service = runtime.c(Service, { optional: true });
                value = runtime.d("direct");
                otherValue = runtime.e("wrapped");
                requiredValue = runtime.e.required();
                selection = runtime.f("");
                requiredSelection = runtime.g.required();
                aliasedSelection = modelAlias("aliased");
                changed = runtime.o();
                opaque = runtime.unknown();
            };
            runtime.ApiCard.compiled = runtime.define({
                type: runtime.ApiCard,
                selectors: [["closure-api-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        runtime.element(0, "section");
                    }
                },
            });
            void publicRuntime;
        }).call(this, this.shared);
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-renamed Angular class APIs should parse");

    assert_eq!(recovered.len(), 1);
    let source = &recovered[0].source;
    assert!(
        source.contains(
            "import { Component, computed, inject, input, model, output, signal } from \"@angular/core\";"
        ),
        "{source}"
    );
    for expected in [
        "count = signal(0)",
        "zeroCount = signal(0)",
        "aliasedZeroCount = signal(0)",
        "label = computed(()=>this.count())",
        "specializedLabel = computed(()=>this.count())",
        "service = inject(Service, {",
        "value = input(\"direct\")",
        "otherValue = input(\"wrapped\")",
        "requiredValue = input.required()",
        "selection = model(\"\")",
        "requiredSelection = model.required()",
        "aliasedSelection = model(\"aliased\")",
        "changed = output()",
        "opaque = globalThis.shared.unknown()",
    ] {
        assert!(source.contains(expected), "missing {expected:?}:\n{source}");
    }
    assert!(!source.contains("this.shared"));
}

#[test]
fn propagates_specialized_signal_arguments_across_an_esm_alias() {
    let runtime = r#"
        function makeZeroSignal() {
            const node = Object.create(signalNode);
            node.value = 0;
            const read = () => {
                trackProducer(node);
                return node.value;
            };
            return read[nodeKey] = node, [
                read,
                (value) => setSignal(node, value),
                (update) => setSignal(node, update(node.value)),
            ];
        }

        export const specializedSignal = () => {
            const [read, set, update] = makeZeroSignal();
            read.set = set;
            read.update = update;
            read.renamedReadonly = asReadonly.bind(read);
            return read;
        };
    "#;
    let component = r#"
        import { specializedSignal as state } from "./runtime.js";
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class SpecializedSignalComponent {
            count = state();

            static compiled = define({
                type: SpecializedSignalComponent,
                selectors: [["specialized-signal"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "section");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_modules(
        &[
            AngularModuleSource {
                filename: "runtime.js",
                source: runtime,
            },
            AngularModuleSource {
                filename: "component.js",
                source: component,
            },
        ],
        AngularRecoveryOptions::default(),
    )
    .expect("the ESM-aliased specialized signal should parse");

    assert_eq!(recovered.len(), 1);
    assert!(
        recovered[0].source.contains("count = signal(0)"),
        "{}",
        recovered[0].source
    );
}

#[test]
fn rejects_incomplete_angular_class_api_lookalikes() {
    let source = r#"
        const runtime = {};

        function makeComputed(computation, equal) {
            const node = Object.create(computedNode);
            node.computation = computation;
            node.equal = equal;
            const read = () => node.value;
            read[nodeKey] = node;
            return read;
        }
        function injectFlags(options) {
            if (typeof options === "undefined" || typeof options === "number") {
                return options;
            }
            return 0 | (options.optional && 8) | (options.self && 2);
        }

        runtime.signalish = (initial, options) => {
            const [read, set, update] = makeSignal(initial, options?.equal);
            read.set = set;
            read.update = update;
            return read;
        };
        runtime.computedish = (computation, options) =>
            makeComputed(computation, options?.equal);
        runtime.injectish = (token, options) =>
            injectImpl(token, injectFlags(options));
        runtime.inputish = (initial, options) => {
            function read() {
                if (node.value === requiredUnset) {
                    throw new RuntimeError(-951, null);
                }
                return node.value;
            }
            const node = Object.create(inputNode);
            node.value = initial;
            node.transform = options?.transform;
            read[nodeKey] = node;
            return read;
        };
        runtime.modelish = (initial) => {
            function read() {
                if (node.value === requiredUnset) {
                    throw new RuntimeError(952, false);
                }
                return node.value;
            }
            const node = Object.create(inputNode);
            node.value = initial;
            read[nodeKey] = node;
            read.asReadonly = asReadonly.bind(read);
            read.set = (value) => setSignal(node, value);
            read.update = (update) => read.set(update(node.value));
            return read;
        };
        class OutputLookalike {
            subscribe(listener) {
                if (this.destroyed) {
                    throw new RuntimeError(954, false);
                }
                return {
                    unsubscribe: () => removeListener(listener),
                };
            }
            emit(value) {
                warnAboutDestroyedOutput(953);
                notifyListeners(value);
            }
        }
        runtime.outputish = () => new OutputLookalike();

        runtime.define = function(definition) {
            return definition;
        };
        runtime.element = function() {
            return runtime.element;
        };
        const publicRuntime = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelement": runtime.element,
        };

        class LookalikeCardComponent {
            count = runtime.signalish(0);
            label = runtime.computedish(() => this.count());
            service = runtime.injectish(Service);
            value = runtime.inputish("reader");
            selection = runtime.modelish("");
            changed = runtime.outputish();
        }
        LookalikeCardComponent.compiled = runtime.define({
            type: LookalikeCardComponent,
            selectors: [["lookalike-card"]],
            template: function(renderFlags) {
                if (renderFlags & 1) {
                    runtime.element(0, "section");
                }
            },
        });
        void publicRuntime;
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("class API lookalikes should parse");

    assert_eq!(recovered.len(), 1);
    let source = &recovered[0].source;
    assert!(source.starts_with("import { Component } from \"@angular/core\";"));
    for expected in [
        "runtime.signalish(0)",
        "runtime.computedish(()=>this.count())",
        "runtime.injectish(Service)",
        "runtime.inputish(\"reader\")",
        "runtime.modelish(\"\")",
        "runtime.outputish()",
    ] {
        assert!(source.contains(expected), "missing {expected:?}:\n{source}");
    }
}

#[test]
fn infers_closure_renamed_multi_value_text_interpolation() {
    let source = r#"
        const runtime = {};
        const noChange = {};

        function currentView() {
            return activeView;
        }
        function selectedNode(view) {
            return view.selected;
        }
        function stringify(value) {
            return String(value);
        }
        function interpolateOne(view, prefix, value, suffix = "") {
            return bindingUpdated(view, value)
                ? prefix + stringify(value) + suffix
                : noChange;
        }
        function interpolateTwo(view, prefix, first, infix, second, suffix = "") {
            const changed =
                bindingUpdated(view, first) | bindingUpdated(view, second);
            return changed
                ? prefix + stringify(first) + infix + stringify(second) + suffix
                : noChange;
        }

        runtime.t1 = function(prefix, value, suffix) {
            const view = currentView();
            const rendered = interpolateOne(view, prefix, value, suffix);
            if (rendered !== noChange) {
                selectedNode(view).nodeValue = rendered;
            }
            return runtime.t1;
        };
        runtime.t0 = function(value) {
            runtime.t1("", value);
            return runtime.t0;
        };
        runtime.t2 = function(prefix, first, infix, second, suffix) {
            const view = currentView();
            const rendered = interpolateTwo(
                view,
                prefix,
                first,
                infix,
                second,
                suffix
            );
            if (rendered !== noChange) {
                selectedNode(view).nodeValue = rendered;
            }
            return runtime.t2;
        };

        runtime.define = function(definition) {
            return definition;
        };
        runtime.text = function() {
            return runtime.text;
        };
        runtime.advance = function() {
            return runtime.advance;
        };
        const publicRuntime = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵtext": runtime.text,
            "ɵɵadvance": runtime.advance,
        };

        class InterpolationCardComponent {
            single = "Title";
            first = "Left";
            second = "Right";

            static compiled = runtime.define({
                type: InterpolationCardComponent,
                selectors: [["interpolation-card"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.text(0);
                        runtime.text(1);
                    }
                    if (renderFlags & 2) {
                        runtime.t0(context.single);
                        runtime.advance(1);
                        runtime.t2(
                            " ",
                            context.first,
                            ": ",
                            context.second,
                            " "
                        );
                    }
                },
            });
        }
        void publicRuntime;
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-renamed text interpolation should parse");

    assert_eq!(recovered.len(), 1);
    let component = &recovered[0];
    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("{{ single }}"));
    assert!(component.source.contains("{{ first }}: {{ second }}"));
    assert!(!component.source.contains("runtime.t2"));
}

#[test]
fn derives_a_readable_component_name_from_the_selector() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        const a = class b {
            static compiled = define({
                type: b,
                selectors: [["project-panel"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "hr");
                    }
                },
            });
        };
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("minified component bindings should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].name, "ProjectPanelComponent");
    assert!(recovered[0]
        .source
        .contains("export class ProjectPanelComponent"));
}

#[test]
fn restores_the_listener_event_parameter_name() {
    let source = r#"
        import * as core from "@angular/core";

        class EventCardComponent {
            change(value) {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: EventCardComponent,
                selectors: [["event-card"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        core.ɵɵelementStart(0, "input");
                        core.ɵɵlistener("input", function(a) {
                            return context.change(a.target.value);
                        });
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("listener parameter recovery should parse");

    assert_eq!(recovered.len(), 1);
    assert!(
        recovered[0]
            .source
            .contains("<input (input)=\"change($event.target.value)\" />"),
        "{}",
        recovered[0].source
    );
}

#[test]
fn accepts_modern_dom_instruction_imports() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵdomElementStart as start,
            ɵɵdomElementEnd as end,
            ɵɵdomListener as listen,
            ɵɵdomProperty as property,
            ɵɵtext as text,
        } from "@angular/core";

        class ModernCardComponent {
            active = false;
            toggle() {
                this.active = !this.active;
            }

            static ɵcmp = define({
                type: ModernCardComponent,
                selectors: [["modern-card"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        start(0, "button");
                        listen("click", function() {
                            return context.toggle();
                        });
                        text(1, "Toggle");
                        end();
                    }
                    if (renderFlags & 2) {
                        property("disabled", context.active);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("modern DOM instruction aliases should parse");

    assert_eq!(recovered.len(), 1);
    assert!(recovered[0]
        .source
        .contains("<button (click)=\"toggle()\" [disabled]=\"active\">Toggle</button>"));
}

#[test]
fn merges_equivalent_instruction_alias_evidence() {
    let source = r#"
        runtime.define = function(definition) {
            return definition;
        };
        runtime.start = function(index, name, attrs, refs) {
            createNode(index, name, attrs, refs);
            return runtime.start;
        };
        runtime.end = function() {
            leaveNode();
            return runtime.end;
        };
        runtime.element = function(index, name, attrs, refs) {
            runtime.start(index, name, attrs, refs);
            runtime.end();
            return runtime.element;
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵdomElementStart": runtime.start,
            "ɵɵdomElementEnd": runtime.end,
            "ɵɵdomElement": runtime.element,
        };

        class AliasEvidenceComponent {}
        AliasEvidenceComponent.compiled = runtime.define({
            type: AliasEvidenceComponent,
            selectors: [["alias-evidence"]],
            template: function(renderFlags) {
                if (renderFlags & 1) {
                    runtime.start(0, "article");
                    runtime.end();
                }
            },
        });
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("equivalent canonical aliases should not conflict");

    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].source.contains("<article></article>"));
}

#[test]
fn recovers_creation_effects_from_a_minified_if_test() {
    let source = r#"
        import * as core from "@angular/core";

        class CombinedPhaseComponent {
            label = "Combined";

            static ɵcmp = core.ɵɵdefineComponent({
                type: CombinedPhaseComponent,
                selectors: [["combined-phase"]],
                template: function(renderFlags, context) {
                    if (
                        (
                            renderFlags & 1 &&
                                (
                                    core.ɵɵelementStart(0, "h2"),
                                    core.ɵɵtext(1),
                                    core.ɵɵelementEnd()
                                ),
                            renderFlags & 2
                        )
                    ) {
                        core.ɵɵadvance();
                        core.ɵɵtextInterpolate(context.label);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("comma-folded render phases should parse");

    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].source.contains("<h2>{{ label }}</h2>"));
}

#[test]
fn rejects_a_shadowed_lookalike_api() {
    let source = r#"
        const core = {
            ɵɵdefineComponent(value) {
                return value;
            },
            ɵɵelement() {},
        };
        class NotAngular {}
        NotAngular.value = core.ɵɵdefineComponent({
            type: NotAngular,
            selectors: [["not-angular"]],
            template(rf) {
                if (rf & 1) core.ɵɵelement(0, "div");
            },
        });
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("lookalike JavaScript should parse");
    assert!(recovered.is_empty());
}

#[test]
fn marks_unsupported_ivy_regions_as_partial() {
    let source = r#"
        import * as core from "@angular/core";
        class LocalizedComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: LocalizedComponent,
                selectors: [["localized-content"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵi18nStart(0, 1);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("unsupported production Ivy should still recover partially");
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Partial
    );
    assert!(recovered[0]
        .source
        .contains("<!-- Unsupported Ivy instruction: ɵɵi18nStart -->"));
}

#[test]
fn recovers_renamed_instructions_and_descriptor_fields_from_export_evidence() {
    let source = r#"
        function d(value) { return value; }
        function s() { return s; }
        function e() { return e; }
        function x() {}
        function a() {}
        function i() {}
        const publicRuntime = {
            "ɵɵdefineComponent": d,
            "ɵɵelementStart": s,
            "ɵɵelementEnd": e,
            "ɵɵtext": x,
            "ɵɵadvance": a,
            "ɵɵtextInterpolate": i,
        };

        class a0 {
            label = "Renamed";
            static q;
        }
        a0.q = d({
            type: a0,
            k: [["renamed-card"]],
            c: [
                ["class", "box"]
            ],
            t: function(r, v) {
                r & 1 && (s(0, "article", 0)(1, "h2"), x(2), e()());
                r & 2 && (a(2), i(v.label));
            },
            z: [
                "article[_ngcontent-%COMP%] { border: 1px solid; }"
            ],
        });
        void publicRuntime;
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("renamed production Ivy should parse");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].selector, "renamed-card");
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert!(recovered[0].source.contains("<article class=\"box\">"));
    assert!(recovered[0].source.contains("<h2>{{ label }}</h2>"));
    assert!(recovered[0]
        .source
        .contains("article { border: 1px solid; }"));
    assert!(!recovered[0].source.contains("static q"));
}

#[test]
fn shares_unresolved_namespace_roles_across_module_sources() {
    let runtime = r#"
        shared.publicRuntime = {
            "ɵɵdefineComponent": shared.d,
            "ɵɵelement": shared.e,
        };
    "#;
    let component = r#"
        class CrossModuleComponent {
            static x;
        }
        CrossModuleComponent.x = shared.d({
            type: CrossModuleComponent,
            p: [["cross-module"]],
            t: function(r) {
                if (r & 1) {
                    shared.e(0, "hr");
                }
            },
        });
    "#;

    let recovered = recover_angular_components_from_modules(
        &[
            AngularModuleSource {
                filename: "runtime.js",
                source: runtime,
            },
            AngularModuleSource {
                filename: "component.js",
                source: component,
            },
        ],
        AngularRecoveryOptions::default(),
    )
    .expect("the generic module workspace should share unresolved symbol roles");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].selector, "cross-module");
    assert!(recovered[0].source.contains("<hr />"));
    assert!(!recovered[0].source.contains("static x"));
}

#[test]
fn matches_a_named_class_expression_by_its_inner_binding() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        const ReadableCardComponent = class a {
            static compiled = define({
                type: a,
                selectors: [["readable-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "hr");
                    }
                },
            });
        };
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("named class expressions should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].name, "ReadableCardComponent");
    assert!(recovered[0]
        .source
        .contains("export class ReadableCardComponent"));
    assert!(recovered[0].source.contains("<hr />"));
}

#[test]
fn follows_named_esm_symbol_edges_across_production_chunks() {
    let runtime = r#"
        function define(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        }
        function element(index, name, attrs, refs) {
            createElement(index, name, attrs, refs);
            return element;
        }
        function text(index, value = "") {
            createText(index, value);
        }
        const publicRuntime = {
            "ɵɵdefineComponent": define,
            "ɵɵelement": element,
        };
        export { define as a, element as b, text as c };
        void publicRuntime;
    "#;
    let component = r#"
        import { a as component, b as node, c as content } from "./runtime.js";

        const ChunkCardComponent = class c {
            static compiled = component({
                type: c,
                selectors: [["chunk-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        node(0, "hr");
                        content(1, "Chunk");
                    }
                },
            });
        };
    "#;

    let recovered = recover_angular_components_from_modules(
        &[
            AngularModuleSource {
                filename: "chunks/runtime.js",
                source: runtime,
            },
            AngularModuleSource {
                filename: "chunks/component.js",
                source: component,
            },
        ],
        AngularRecoveryOptions::default(),
    )
    .expect("named ESM edges should resolve across the module workspace");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].name, "ChunkCardComponent");
    assert_eq!(recovered[0].selector, "chunk-card");
    assert!(recovered[0].source.contains("<hr />\n    Chunk"));
}

#[test]
fn parallel_preparation_keeps_same_named_module_bindings_distinct() {
    let runtime = r#"
        function define(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
            }));
        }
        function element(index, name, attrs, refs) {
            createElement(index, name, attrs, refs);
            return element;
        }
        const publicRuntime = {
            "ɵɵdefineComponent": define,
            "ɵɵelement": element,
        };
        export { define as a, element as b };
        void publicRuntime;
    "#;
    let component = r#"
        import { a as x, b as y } from "./runtime.js";

        class c {
            static compiled = x({
                type: c,
                selectors: [["parallel-card"]],
                template: function(rf) {
                    if (rf & 1) y(0, "main");
                },
            });
        }
    "#;
    let same_named_decoy = r#"
        function x(value) {
            return value;
        }
        function y() {}
        class c {
            static compiled = x({
                type: c,
                selectors: [["not-angular"]],
                template: function(rf) {
                    if (rf & 1) y(0, "aside");
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_modules(
        &[
            AngularModuleSource {
                filename: "runtime.js",
                source: runtime,
            },
            AngularModuleSource {
                filename: "component.js",
                source: component,
            },
            AngularModuleSource {
                filename: "decoy.js",
                source: same_named_decoy,
            },
        ],
        AngularRecoveryOptions::default(),
    )
    .expect("parallel module preparation should preserve binding identity");

    assert_eq!(report.components.len(), 1);
    assert_eq!(report.components[0].selector, "parallel-card");
    assert!(report.components[0].source.contains("<main></main>"));
}

#[test]
fn profiling_spans_separate_angular_preparation_inference_and_recovery() {
    let (report, spans) = crate::test_tracing::record_spans(|| {
        analyze_angular_components_from_module_views(
            &[AngularModuleView {
                filename: "profiled.js",
                evidence_source: PRODUCTION_COMPONENT,
                readable_source: PRODUCTION_COMPONENT,
            }],
            AngularRecoveryOptions::default(),
        )
        .expect("the profiled component should recover")
    });

    assert_eq!(report.components.len(), 1);
    for expected in [
        "angular: prepare module views",
        "angular: recover prepared modules",
        "angular: infer Ivy roles",
        "angular: index artifact symbols",
        "angular: recover components",
    ] {
        assert!(
            spans.iter().any(|span| span == expected),
            "missing {expected:?} in {spans:#?}"
        );
    }
}

#[test]
fn rejects_conflicting_export_role_evidence() {
    let source = r#"
        function ambiguous() {}
        const publicRuntime = {
            "ɵɵdefineComponent": ambiguous,
            "ɵɵelement": ambiguous,
        };
        class AmbiguousComponent {}
        AmbiguousComponent.value = ambiguous({
            type: AmbiguousComponent,
            p: [["ambiguous-component"]],
            t: function(r) {
                if (r & 1) ambiguous(0, "div");
            },
        });
        void publicRuntime;
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("conflicting role evidence should not make parsing fail");
    assert!(recovered.is_empty());
}

#[test]
fn recovers_a_component_class_assigned_to_a_namespace_member() {
    let source = r#"
        function define(value) { return value; }
        function element() { return element; }
        const publicRuntime = {
            "ɵɵdefineComponent": define,
            "ɵɵelement": element,
        };

        scope.NamespaceCard = class {
            title = "Namespaced";
            static metadata;
        };
        scope.NamespaceCard.metadata = define({
            type: scope.NamespaceCard,
            selectors: [["namespace-card"]],
            template: function(renderFlags) {
                if (renderFlags & 1) {
                    element(0, "section");
                }
            },
        });
        void publicRuntime;
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("namespace-assigned classes should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].name, "NamespaceCard");
    assert!(recovered[0].source.contains("<section></section>"));
    assert!(!recovered[0].source.contains("static metadata"));
}

#[test]
fn infers_renamed_component_and_element_helpers_from_runtime_shapes() {
    let source = r#"
        runtime.component = function(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        };
        runtime.start = function(index, name, attrs, refs) {
            createNode(index, name, attrs, refs);
            return runtime.start;
        };
        runtime.end = function() {
            leaveNode();
            return runtime.end;
        };
        runtime.element = function(index, name, attrs, refs) {
            runtime.start(index, name, attrs, refs);
            runtime.end();
            return runtime.element;
        };

        scope.StructuralCard = class {
            label = "Structural";
            static compiled;
        };
        scope.StructuralCard.compiled = runtime.component({
            type: scope.StructuralCard,
            x: [["structural-card"]],
            template: function(renderFlags) {
                if (renderFlags & 1) {
                    runtime.element(0, "article");
                }
            },
            styles: ["article { display: block; }"],
        });
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("structural runtime evidence should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].name, "StructuralCard");
    assert_eq!(recovered[0].selector, "structural-card");
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert!(recovered[0].source.contains("<article></article>"));
    assert!(recovered[0].source.contains("article { display: block; }"));
    assert!(!recovered[0].source.contains("static compiled"));
}

#[test]
fn does_not_infer_a_runtime_helper_after_a_non_function_reassignment() {
    let source = r#"
        runtime.component = function(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        };
        runtime.component = unrelated;

        class ReassignedComponent {}
        ReassignedComponent.compiled = runtime.component({
            type: ReassignedComponent,
            selectors: [["reassigned-card"]],
            template: function() {},
        });
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("reassigned runtime helpers should parse");

    assert!(
        recovered.is_empty(),
        "a stale structural definition must not classify the reassigned helper"
    );
}

#[test]
fn does_not_trust_export_map_evidence_after_a_binding_reassignment() {
    let source = r#"
        function define(definition) {
            return definition;
        }
        const publicRuntime = {
            "ɵɵdefineComponent": define,
        };
        define = unrelated;

        class ReassignedExportComponent {}
        ReassignedExportComponent.compiled = define({
            type: ReassignedExportComponent,
            selectors: [["reassigned-export-card"]],
            template: function() {},
        });
        void publicRuntime;
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("reassigned export-map helpers should parse");

    assert!(
        recovered.is_empty(),
        "export-map evidence must not survive reassignment of its value"
    );
}

#[test]
fn does_not_treat_a_logical_assignment_as_a_direct_helper_definition() {
    let source = r#"
        let define;
        define ||= function(definition) {
            return definition;
        };
        const publicRuntime = {
            "ɵɵdefineComponent": define,
        };

        class ConditionalDefinitionComponent {}
        ConditionalDefinitionComponent.compiled = define({
            type: ConditionalDefinitionComponent,
            selectors: [["conditional-definition-card"]],
            template: function() {},
        });
        void publicRuntime;
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("logical helper assignments should parse");

    assert!(
        recovered.is_empty(),
        "a logical assignment does not prove which helper value is installed"
    );
}

#[test]
fn does_not_infer_self_returning_helpers_from_nested_return_values() {
    let source = r#"
        function define(definition) {
            return definition;
        }
        const publicRuntime = {
            "ɵɵdefineComponent": define,
        };
        runtime.start = function(index, name, attrs, refs) {
            createNode(index, name, attrs, refs);
            return wrap(runtime.start);
        };
        runtime.end = function() {
            leaveNode();
            return runtime.end;
        };
        runtime.element = function(index, name, attrs, refs) {
            runtime.start(index, name, attrs, refs);
            runtime.end();
            return runtime.element;
        };

        class NestedReturnComponent {}
        NestedReturnComponent.compiled = define({
            type: NestedReturnComponent,
            selectors: [["nested-return-card"]],
            template: function(renderFlags) {
                if (renderFlags & 1) {
                    runtime.element(0, "article");
                }
            },
        });
        void publicRuntime;
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("nested self-return lookalikes should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Partial
    );
    assert!(!recovered[0].source.contains("<article></article>"));
}

#[test]
fn infers_a_component_helper_after_object_merge_lowering() {
    let source = r#"
        runtime.component = function(definition) {
            return noSideEffects(() => {
                const base = directiveDefinition(definition);
                const result = merge(copy({}, base), {
                    decls: definition.decls,
                    vars: definition.vars,
                    template: definition.template,
                    consts: definition.consts,
                    dependencies: definition.dependencies,
                    styles: definition.styles,
                });
                finalizeDefinition(result);
                return result;
            });
        };
        runtime.element = function(index, name, attrs, refs) {
            createNode(index, name, attrs, refs);
            return runtime.element;
        };
        runtime.public = {
            "ɵɵelement": runtime.element,
        };

        const LoweredCardComponent = class c {
            static compiled = runtime.component({
                type: c,
                selectors: [["lowered-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        runtime.element(0, "hr");
                    }
                },
            });
        };
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("lowered descriptor construction should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].selector, "lowered-card");
    assert!(recovered[0].source.contains("<hr />"));
}

#[test]
fn infers_a_specialized_element_pair_from_proven_template_use() {
    let source = r#"
        runtime.component = function(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        };
        runtime.start = function(index, name, attrs, refs) {
            createSpecializedNode(index, name, attrs, refs);
            return runtime.start;
        };
        runtime.end = function() {
            leaveSpecializedNode();
            return runtime.end;
        };
        const SpecializedCardComponent = class c {
            static compiled = runtime.component({
                type: c,
                selectors: [["specialized-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        runtime.start(0, "article");
                        runtime.end();
                    }
                },
            });
        };
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("template use should prove a specialized element pair");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].selector, "specialized-card");
    assert!(recovered[0].source.contains("<article></article>"));
}

#[test]
fn infers_a_specialized_element_start_with_a_minified_tracing_branch() {
    let source = r#"
        runtime.component = function(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        };
        runtime.start = function(index, name, attrs, refs) {
            if (tracing && tracing.enabled) {
                return tracing.a(node(index), () => {
                    createSpecializedNode(index, name, attrs, refs);
                    return runtime.start;
                });
            }
            createSpecializedNode(index, name, attrs, refs);
            return runtime.start;
        };
        runtime.end = function() {
            leaveSpecializedNode();
            return runtime.end;
        };
        runtime.element = function(index, name, attrs, refs) {
            runtime.start(index, name, attrs, refs);
            runtime.end();
            return runtime.element;
        };

        const TracedSpecializedCardComponent = class c {
            static compiled = runtime.component({
                type: c,
                selectors: [["traced-specialized-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        runtime.element(0, "article");
                    }
                },
            });
        };
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("a minified tracing branch should retain the specialized element pair");

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        recovered[0].issues,
        recovered[0].source,
    );
    assert!(recovered[0].source.contains("<article></article>"));
}

#[test]
fn rejects_a_config_normalizer_that_only_resembles_component_definition() {
    let source = r#"
        runtime.normalize = function(config) {
            return {
                template: config.template,
                dependencies: config.dependencies,
                styles: config.styles,
            };
        };
        scope.Unrelated = class {};
        scope.Unrelated.value = runtime.normalize({
            type: scope.Unrelated,
            selectors: [["unrelated-value"]],
            template: function() {},
            dependencies: [],
            styles: [],
        });
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("lookalike configuration code should parse");
    assert!(recovered.is_empty());
}

#[test]
fn marks_unclassified_calls_on_a_proven_runtime_namespace_as_partial() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelement": runtime.element,
        };
        class ConservativeComponent {}
        ConservativeComponent.compiled = runtime.define({
            type: ConservativeComponent,
            selectors: [["conservative-card"]],
            template: function(renderFlags) {
                if (renderFlags & 1) {
                    runtime.element(0, "article");
                    runtime.q(1)(2, 3);
                }
            },
        });
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("unclassified runtime calls should not prevent partial recovery");
    let recovered = &report.components;

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Partial
    );
    assert!(recovered[0]
        .source
        .contains("Unsupported Ivy instruction: unknown-runtime-instruction"));
    assert_eq!(
        recovered[0]
            .source
            .matches("Unsupported Ivy instruction: unknown-runtime-instruction")
            .count(),
        1,
        "equivalent human-readable issue comments should be emitted once"
    );
    assert!(!recovered[0].source.contains("runtime.q"));
    assert_eq!(
        recovered[0].unknown_runtime_call_shapes,
        vec![AngularUnknownRuntimeCallShape {
            phase: AngularTemplatePhase::Creation,
            argument_counts: vec![1, 2],
            occurrences: 1,
            runtime_calls: 2,
        }]
    );
    assert_eq!(
        report.unknown_runtime_call_shapes,
        recovered[0].unknown_runtime_call_shapes
    );
}

#[test]
fn infers_text_listener_and_advance_from_runtime_and_template_shapes() {
    let source = r#"
        runtime.component = function(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        };
        runtime.start = function(index, name, attrs, refs) {
            createNode(index, name, attrs, refs);
            return runtime.start;
        };
        runtime.end = function() {
            leaveNode();
            return runtime.end;
        };
        runtime.element = function(index, name, attrs, refs) {
            runtime.start(index, name, attrs, refs);
            runtime.end();
            return runtime.element;
        };
        runtime.text = function(index, value = "") {
            createText(index, value);
        };
        runtime.listen = function(eventName, callback, target) {
            addListener(eventName, callback, target);
            return runtime.listen;
        };
        runtime.next = function(delta = 1) {
            selectIndex(currentIndex() + delta);
        };

        class InteractiveCard {
            select() {}
        }
        InteractiveCard.compiled = runtime.component({
            type: InteractiveCard,
            x: [["interactive-card"]],
            template: function(renderFlags, context) {
                if (renderFlags & 1) {
                    runtime.start(0, "button");
                    runtime.listen("click", function() {
                        return context.select();
                    });
                    runtime.text(1, "Choose");
                    runtime.end();
                }
                if (renderFlags & 2) {
                    runtime.next();
                }
            },
        });
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("template-informed structural roles should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert!(recovered[0]
        .source
        .contains("<button (click)=\"select()\">Choose</button>"));
    assert!(!recovered[0].source.contains("unknown-runtime-instruction"));
}

#[test]
fn infers_text_interpolation_and_property_binding_relationships() {
    let source = r#"
        runtime.component = function(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        };
        runtime.start = function(index, name, attrs, refs) {
            createNode(index, name, attrs, refs);
            return runtime.start;
        };
        runtime.end = function() {
            leaveNode();
            return runtime.end;
        };
        runtime.element = function(index, name, attrs, refs) {
            runtime.start(index, name, attrs, refs);
            runtime.end();
            return runtime.element;
        };
        runtime.text = function(index, value = "") {
            createText(index, value);
        };
        runtime.next = function(delta = 1) {
            selectIndex(currentIndex() + delta);
        };
        runtime.interpolateOne = function(prefix, value, suffix) {
            const view = getView();
            const rendered = interpolateValue(view, prefix, value, suffix);
            if (rendered !== noChange) {
                updateText(view, getSelectedIndex(), rendered);
            }
            return runtime.interpolateOne;
        };
        runtime.interpolate = function(value) {
            runtime.interpolateOne("", value);
            return runtime.interpolate;
        };
        runtime.property = function(name, value, sanitizer) {
            const view = getView();
            const binding = nextBindingIndex();
            if (bindingChanged(view, binding, value)) {
                const node = getSelectedNode();
                writeProperty(node, view, name, value, view[0], sanitizer);
            }
            return runtime.property;
        };
        runtime.style = function(name, value, suffix) {
            writeStyle(name, value, suffix, false);
            return runtime.style;
        };

        function InitializerPropertyHost(renderFlags, context) {
            if (renderFlags & 1) {
                runtime.start(0, "div");
                runtime.end();
            }
            if (renderFlags & 2) {
                const chain = runtime.property("title", context.label);
            }
        }

        class BoundCard {
            label = "Bound";
            disabled = false;
        }
        BoundCard.compiled = runtime.component({
            type: BoundCard,
            x: [["bound-card"]],
            template: function(renderFlags, context) {
                if (renderFlags & 1) {
                    runtime.start(0, "article");
                    runtime.text(1);
                    runtime.end();
                    runtime.element(2, "button");
                }
                if (renderFlags & 2) {
                    runtime.next();
                    runtime.interpolate(context.label);
                    runtime.next();
                    runtime.property("disabled", context.disabled);
                }
            },
        });
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("relationship-backed binding roles should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert!(recovered[0]
        .source
        .contains("<article>{{ label }}</article>"));
    assert!(recovered[0]
        .source
        .contains("<button [disabled]=\"disabled\"></button>"));
    assert!(!recovered[0].source.contains("[style.disabled]"));
}

#[test]
fn infers_a_specialized_property_from_an_ordered_renderer_write() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
        };
        runtime.property = function(name, value, sanitizer) {
            const view = currentView();
            const binding = nextBindingIndex();
            if (bindingChanged(view, binding, value)) {
                const renderer = getRenderer(view);
                const node = getSelectedNode(view);
                renderer.setProperty(
                    node,
                    name,
                    sanitizer ? sanitizer(value) : value
                );
            }
            return runtime.property;
        };
        runtime.lookalike = function(name, value, sanitizer) {
            const view = currentView();
            const binding = nextBindingIndex();
            if (bindingChanged(view, binding, value)) {
                const renderer = getRenderer(view);
                const node = getSelectedNode(view);
                renderer.setProperty(node, value, name);
            }
            return runtime.lookalike;
        };

        class PropertyComponent {
            disabled = false;

            static compiled = runtime.component({
                type: PropertyComponent,
                selectors: [["property-card"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        runtime.start(0, "button");
                        runtime.end();
                    }
                    if (rf & 2) {
                        runtime.property("disabled", context.disabled);
                    }
                },
            });
        }

        class PropertyLookalikeComponent {
            disabled = false;

            static compiled = runtime.component({
                type: PropertyLookalikeComponent,
                selectors: [["property-lookalike"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        runtime.start(0, "button");
                        runtime.end();
                    }
                    if (rf & 2) {
                        runtime.lookalike("disabled", context.disabled);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("specialized property roles should be analyzed");
    let property = report
        .components
        .iter()
        .find(|component| component.selector == "property-card")
        .expect("the property component should recover");
    let lookalike = report
        .components
        .iter()
        .find(|component| component.selector == "property-lookalike")
        .expect("the lookalike component should remain visible");

    assert_eq!(
        property.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        property.issues,
        property.source,
    );
    assert!(property.source.contains("[disabled]=\"disabled\""));
    assert_eq!(lookalike.completeness, AngularRecoveryCompleteness::Partial);
    assert!(lookalike
        .source
        .contains("Unsupported Ivy instruction: unknown-runtime-instruction"));
}

#[test]
fn uses_pre_rewrite_evidence_with_the_readable_class_view() {
    let evidence = r#"
        const decorate = (value) => "before helper " + value;

        runtime.component = function(definition) {
            return noSideEffects(() => Object.assign({}, baseDefinition, {
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        };
        runtime.element = function(index, name, attrs, refs) {
            createElement(index, name, attrs, refs);
            return runtime.element;
        };
        runtime.public = {
            "ɵɵelement": runtime.element,
        };

        class PipelineCardComponent {
            label = decorate("before rewrites");

            static compiled = runtime.component({
                type: PipelineCardComponent,
                selectors: [["pipeline-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        runtime.element(0, "article");
                    }
                },
                dependencies: [],
                styles: [],
            });
        }
    "#;
    let readable = r#"
        const decorate = (value) => "after helper " + value;

        runtime.component = function(definition) {
            return noSideEffects(() => ({
                ...baseDefinition,
                type: definition.type,
                selectors: definition.selectors,
                template: definition.template,
                dependencies: definition.dependencies,
                styles: definition.styles,
            }));
        };
        runtime.element = function(index, name, attrs, refs) {
            createElement(index, name, attrs, refs);
            return runtime.element;
        };
        runtime.public = {
            "ɵɵelement": runtime.element,
        };

        class PipelineCardComponent {
            label = decorate("after rewrites");

            static compiled = runtime.component({
                type: PipelineCardComponent,
                selectors: [["pipeline-card"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        runtime.element(0, "article");
                    }
                },
                dependencies: [],
                styles: [],
            });
        }
    "#;

    let recovered = recover_angular_components_from_module_views(
        &[AngularModuleView {
            filename: "pipeline-card.js",
            evidence_source: evidence,
            readable_source: readable,
        }],
        AngularRecoveryOptions::default(),
    )
    .expect("the evidence and readable views should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].selector, "pipeline-card");
    assert!(recovered[0].source.contains("<article"));
    assert!(recovered[0].source.contains("\"after helper \" + value"));
    assert!(!recovered[0].source.contains("\"before helper \" + value"));
    assert!(recovered[0].source.contains("\"after rewrites\""));
    assert!(!recovered[0].source.contains("\"before rewrites\""));
}

#[test]
fn reports_accounting_for_every_rendered_instruction_call() {
    let report =
        analyze_angular_components_from_js(PRODUCTION_COMPONENT, AngularRecoveryOptions::default())
            .expect("production Ivy should be analyzed");

    assert_eq!(report.components.len(), 1);
    assert_eq!(report.stats.modules_analyzed, 1);
    assert_eq!(report.stats.component_candidates, 1);
    assert_eq!(report.stats.recovered_components, 1);
    assert_eq!(report.stats.rejected_component_candidates, 0);
    assert_eq!(report.stats.complete_components, 1);
    assert_eq!(report.stats.partial_components, 0);
    assert_eq!(report.stats.runtime_calls_observed, 13);
    assert_eq!(report.stats.rendered_instruction_calls, 13);
    assert_eq!(report.stats.unsupported_runtime_calls, 0);
    assert_eq!(report.stats.malformed_instruction_calls, 0);
    assert!(report.components[0].issues.is_empty());
    assert_eq!(
        report.components[0].stats,
        AngularTemplateRecoveryStats {
            runtime_calls_observed: 13,
            rendered_instruction_calls: 13,
            unsupported_runtime_calls: 0,
            malformed_instruction_calls: 0,
        }
    );
}

#[test]
fn unsupported_template_statements_make_recovery_partial() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class StatementComponent {
            static ɵcmp = define({
                type: StatementComponent,
                selectors: [["statement-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        const ignored = sideEffect();
                        element(0, "section");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the component should still be recovered");

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Partial
    );
    assert!(recovered[0].issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::UnsupportedStatement
            && issue.detail.as_deref() == Some("declaration")
    }));
    assert!(recovered[0]
        .source
        .contains("<!-- Unsupported Ivy statement: declaration -->"));
    assert!(recovered[0].source.contains("<section></section>"));
}

#[test]
fn malformed_instruction_arguments_make_recovery_partial() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class MalformedComponent {
            static ɵcmp = define({
                type: MalformedComponent,
                selectors: [["malformed-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        element("zero", "section");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the component should still be recovered");

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Partial
    );
    assert_eq!(recovered[0].stats.runtime_calls_observed, 1);
    assert_eq!(recovered[0].stats.rendered_instruction_calls, 0);
    assert_eq!(recovered[0].stats.malformed_instruction_calls, 1);
    let issue = recovered[0]
        .issues
        .iter()
        .find(|issue| {
            issue.kind == AngularRecoveryIssueKind::MalformedInstruction
                && issue.instruction.as_deref() == Some("ɵɵelement")
        })
        .expect("the malformed element should have a structured issue");
    assert_eq!(issue.module_index, Some(0));
    assert_eq!(issue.component.as_deref(), Some("MalformedComponent"));
    assert_eq!(issue.view_id, Some(0));
    assert_eq!(issue.phase, Some(AngularTemplatePhase::Creation));
    assert_eq!(issue.operation_index, Some(0));
    assert_eq!(issue.actual_callee.as_deref(), Some("element"));
    let range = issue
        .source_range
        .expect("the malformed call should retain its source range");
    assert_eq!(
        &source[range.start as usize..range.end as usize],
        r#"element("zero", "section")"#
    );
}

#[test]
fn places_malformed_creation_regions_at_their_structural_location() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class LocatedIssueComponent {
            static ɵcmp = define({
                type: LocatedIssueComponent,
                selectors: [["located-issue"]],
                template: function(rf) {
                    if (rf & 1) {
                        element(0, "header");
                        element("one", "section");
                        element(2, "footer");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the malformed region should remain visible");
    let component = &recovered[0];
    let before = component
        .source
        .find("<header></header>")
        .expect("the preceding sibling should render");
    let issue = component
        .source
        .find("<!-- Malformed Ivy instruction: ɵɵelement")
        .expect("the malformed call should render as an unsupported region");
    let after = component
        .source
        .find("<footer></footer>")
        .expect("the following sibling should render");

    assert!(before < issue && issue < after, "{}", component.source);
    assert!(!component
        .source
        .contains("placement unknown within this view"));
}

#[test]
fn diagnostics_preserve_occurrences_and_view_local_provenance() {
    let source = r#"
        import * as core from "@angular/core";

        function ChildView(rf) {
            if (rf & 1) {
                core.unknownRuntime(0);
                core.ɵɵelement(0, "span");
            }
        }

        class DiagnosticComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: DiagnosticComponent,
                selectors: [["diagnostic-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵtemplate(0, ChildView, 1, 0, "span");
                        core.unknownRuntime(1);
                        core.unknownRuntime(2);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_modules(
        &[AngularModuleSource {
            filename: "diagnostic-card.js",
            source,
        }],
        AngularRecoveryOptions::default(),
    )
    .expect("the partial component should still be analyzed");
    let component = &report.components[0];
    let issues = component
        .issues
        .iter()
        .filter(|issue| issue.kind == AngularRecoveryIssueKind::UnknownRuntimeInstruction)
        .collect::<Vec<_>>();

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert_eq!(issues.len(), 3, "issues: {:#?}", component.issues);
    assert!(issues.iter().all(|issue| {
        issue.module_index == Some(0)
            && issue.component.as_deref() == Some("DiagnosticComponent")
            && issue.phase == Some(AngularTemplatePhase::Creation)
            && issue.actual_callee.as_deref() == Some("core.unknownRuntime")
    }));
    assert_eq!(
        issues
            .iter()
            .filter(|issue| issue.view_id == Some(0))
            .map(|issue| issue.operation_index)
            .collect::<Vec<_>>(),
        vec![Some(1), Some(2)]
    );
    assert_eq!(
        issues
            .iter()
            .filter(|issue| issue.view_id == Some(1))
            .map(|issue| issue.operation_index)
            .collect::<Vec<_>>(),
        vec![Some(0)]
    );
    let mut observed_calls = issues
        .iter()
        .map(|issue| {
            let range = issue
                .source_range
                .expect("each parsed call should retain a source range");
            source[range.start as usize..range.end as usize].to_string()
        })
        .collect::<Vec<_>>();
    observed_calls.sort();
    assert_eq!(
        observed_calls,
        vec![
            "core.unknownRuntime(0)",
            "core.unknownRuntime(1)",
            "core.unknownRuntime(2)",
        ]
    );
    assert_eq!(
        component
            .source
            .matches("Unsupported Ivy instruction: unknown-runtime-instruction")
            .count(),
        1,
        "display comments should remain deduplicated"
    );
    let child_start = component
        .source
        .find("<ng-template>")
        .expect("the child view should render");
    let issue = component
        .source
        .find("<!-- Unsupported Ivy instruction: unknown-runtime-instruction -->")
        .expect("the unsupported child operation should remain visible");
    let child_end = component
        .source
        .find("</ng-template>")
        .expect("the child view should close");
    assert!(
        child_start < issue && issue < child_end,
        "the deduplicated warning should stay inside the smallest affected view:\n{}",
        component.source
    );
    assert!(component
        .source
        .contains("<!-- Wakaru: placement unknown within this view -->"));
}

#[test]
fn report_counts_rejected_component_descriptors() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class ValidComponent {
            static ɵcmp = define({
                type: ValidComponent,
                selectors: [["valid-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        element(0, "div");
                    }
                },
            });
        }

        class RejectedComponent {
            static ɵcmp = define({
                type: RejectedComponent,
                template: function(rf) {
                    if (rf & 1) {
                        element(0, "div");
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the workspace should be analyzed");

    assert_eq!(report.stats.component_candidates, 2);
    assert_eq!(report.stats.recovered_components, 1);
    assert_eq!(report.stats.rejected_component_candidates, 1);
    assert_eq!(report.components[0].selector, "valid-card");
}

#[test]
fn recovers_nested_if_else_views_as_angular_control_flow() {
    let source = r#"
        import * as core from "@angular/core";

        function DetailsIfTemplate(rf, context) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "p", 0);
                core.ɵɵtext(1);
                core.ɵɵelementEnd();
            }
            if (rf & 2) {
                const parent = core.ɵɵnextContext();
                core.ɵɵadvance();
                core.ɵɵtextInterpolate(parent.detail);
            }
        }

        function DetailsElseTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "p", 0);
                core.ɵɵtext(1, "Details hidden");
                core.ɵɵelementEnd();
            }
        }

        class NestedComponent {
            showDetails = true;
            detail = "Nested control flow";

            static ɵcmp = core.ɵɵdefineComponent({
                type: NestedComponent,
                selectors: [["nested-card"]],
                consts: [[1, "details"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵelementStart(0, "article");
                        core.ɵɵtemplate(1, DetailsIfTemplate, 2, 1, "p", 0)(
                            2,
                            DetailsElseTemplate,
                            2,
                            0,
                            "p",
                            0
                        );
                        core.ɵɵelementEnd();
                    }
                    if (rf & 2) {
                        core.ɵɵadvance();
                        core.ɵɵconditional(context.showDetails ? 1 : 2);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("nested Ivy views should be analyzed");

    assert_eq!(report.components.len(), 1);
    assert_eq!(
        report.components[0].completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert!(report.components[0].source.contains("@if (showDetails) {"));
    assert!(report.components[0]
        .source
        .contains(r#"<p class="details">{{ detail }}</p>"#));
    assert!(report.components[0].source.contains("@else {"));
    assert!(report.components[0]
        .source
        .contains(r#"<p class="details">Details hidden</p>"#));
    assert!(!report.components[0].source.contains("<ng-template"));
    assert_eq!(report.stats.runtime_calls_observed, 15);
    assert_eq!(report.stats.rendered_instruction_calls, 15);
    assert_eq!(report.stats.unsupported_runtime_calls, 0);
    assert_eq!(report.stats.malformed_instruction_calls, 0);
}

#[test]
fn infers_an_embedded_template_continuation_from_shared_forwarding() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
            "ɵɵconditional": runtime.conditional,
        };
        runtime.firstTemplate = function(index, template, decls, vars, tag, attrs) {
            controlFlowMarker();
            createTemplate(
                currentView(),
                index,
                template,
                decls,
                vars,
                tag,
                attrs,
                256
            );
            return runtime.nextTemplate;
        };
        runtime.nextTemplate = function(
            index,
            template,
            decls,
            vars,
            tag,
            attrs,
            localRefs,
            extractor
        ) {
            controlFlowMarker();
            createTemplate(
                currentView(),
                index,
                template,
                decls,
                vars,
                tag,
                attrs,
                512,
                localRefs,
                extractor
            );
            return runtime.nextTemplate;
        };
        runtime.emptyFirstTemplate = function() {
            createNext(currentView());
            return runtime.nextTemplate;
        };

        function DetailsTemplate(rf) {
            if (rf & 1) {
                runtime.start(0, "p");
                runtime.text(1, "Details");
                runtime.end();
            }
        }

        function EmptyTemplate(rf) {
            if (rf & 1) {
                runtime.start(0, "p");
                runtime.text(1, "Empty");
                runtime.end();
            }
        }

        class ContinuationComponent {
            showDetails = true;

            static compiled = runtime.component({
                type: ContinuationComponent,
                selectors: [["continuation-card"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        runtime.start(0, "article");
                        runtime.firstTemplate(
                            1,
                            DetailsTemplate,
                            2,
                            0,
                            "p",
                            null
                        )(
                            2,
                            EmptyTemplate,
                            2,
                            0,
                            "p",
                            null
                        );
                        runtime.end();
                    }
                    if (rf & 2) {
                        runtime.conditional(context.showDetails ? 1 : 2);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("template continuation roles should be analyzed");
    let component = &report.components[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@if (showDetails) {"));
    assert!(component.source.contains("<p>Details</p>"));
    assert!(component.source.contains("@else {"));
    assert!(component.source.contains("<p>Empty</p>"));
}

#[test]
fn recovers_canonical_defer_views_and_idle_trigger() {
    let source = r#"
        import * as core from "@angular/core";

        function PrimaryTemplate(rf, context) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "article");
                core.ɵɵtext(1);
                core.ɵɵelementEnd();
            }
            if (rf & 2) {
                const parent = core.ɵɵnextContext();
                core.ɵɵadvance();
                core.ɵɵtextInterpolate(parent.title);
            }
        }

        function LoadingTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "p");
                core.ɵɵtext(1, "Loading");
                core.ɵɵelementEnd();
            }
        }

        function PlaceholderTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "p");
                core.ɵɵtext(1, "Waiting");
                core.ɵɵelementEnd();
            }
        }

        function ErrorTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "p");
                core.ɵɵtext(1, "Failed");
                core.ɵɵelementEnd();
            }
        }

        class DeferredComponent {
            title = "Deferred";

            static compiled = core.ɵɵdefineComponent({
                type: DeferredComponent,
                selectors: [["deferred-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵtemplate(0, PrimaryTemplate, 2, 1);
                        core.ɵɵtemplate(1, LoadingTemplate, 2, 0);
                        core.ɵɵtemplate(2, PlaceholderTemplate, 2, 0);
                        core.ɵɵtemplate(3, ErrorTemplate, 2, 0);
                        core.ɵɵdefer(4, 0, null, 1, 2, 3);
                        core.ɵɵdeferOnIdle();
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("canonical deferred views should be analyzed");
    let component = &report.components[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@defer (on idle) {"));
    assert!(component.source.contains("<article>{{ title }}</article>"));
    assert!(component.source.contains("@loading {"));
    assert!(component.source.contains("<p>Loading</p>"));
    assert!(component.source.contains("@placeholder {"));
    assert!(component.source.contains("<p>Waiting</p>"));
    assert!(component.source.contains("@error {"));
    assert!(component.source.contains("<p>Failed</p>"));
    assert!(!component.source.contains("<ng-template"));
    assert_eq!(
        component.stats.runtime_calls_observed,
        component.stats.rendered_instruction_calls
    );
}

#[test]
fn rejects_a_canonical_defer_call_without_dependency_metadata() {
    let source = r#"
        import * as core from "@angular/core";

        function PrimaryTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "article");
            }
        }

        class ShortDeferComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: ShortDeferComponent,
                selectors: [["short-defer"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵtemplate(0, PrimaryTemplate, 1, 0);
                        core.ɵɵdefer(1, 0);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the malformed defer component should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MalformedInstruction
            && issue.instruction.as_deref() == Some("ɵɵdefer")
            && issue.detail.as_deref() == Some("expected defer metadata arguments")
    }));
    assert!(!component.source.contains("@defer"));
    assert!(component.source.contains("<ng-template>"));
}

#[test]
fn does_not_treat_a_shadowed_undefined_defer_slot_as_nullish() {
    let source = r#"
        import * as core from "@angular/core";

        function PrimaryTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "article");
            }
        }

        class ShadowedUndefinedComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: ShadowedUndefinedComponent,
                selectors: [["shadowed-undefined"]],
                template: function(rf, undefined) {
                    if (rf & 1) {
                        core.ɵɵtemplate(0, PrimaryTemplate, 1, 0);
                        core.ɵɵdefer(1, 0, null, undefined);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the shadowed undefined component should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MalformedInstruction
            && issue.instruction.as_deref() == Some("ɵɵdefer")
            && issue.detail.as_deref() == Some("defer child-template index is not numeric or null")
    }));
    assert!(!component.source.contains("@defer"));
}

#[test]
fn shadowed_undefined_does_not_prove_a_renamed_defer_role() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelement": runtime.element,
            "ɵɵtemplate": runtime.template,
        };
        runtime.defer = function(
            index,
            primary,
            dependencies,
            loading,
            placeholder,
            error,
            loadingConfig,
            placeholderConfig,
            timers,
            flags
        ) {
            markFeature("NgDefer");
            createDefer(
                index,
                primary,
                dependencies,
                loading,
                placeholder,
                error,
                loadingConfig,
                placeholderConfig,
                timers,
                flags
            );
        };

        function PrimaryTemplate(rf) {
            if (rf & 1) {
                runtime.element(0, "article");
            }
        }

        class RenamedShadowedUndefinedComponent {
            static compiled = runtime.component({
                type: RenamedShadowedUndefinedComponent,
                selectors: [["renamed-shadowed-undefined"]],
                template: function(rf, undefined) {
                    if (rf & 1) {
                        runtime.template(0, PrimaryTemplate, 1, 0);
                        runtime.defer(1, 0, null, undefined);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the renamed shadowed undefined component should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::UnknownRuntimeInstruction
            && issue.actual_callee.as_deref() == Some("runtime.defer")
    }));
    assert!(!component.source.contains("@defer"));
}

#[test]
fn infers_closure_renamed_defer_and_self_returning_template_roles() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
            "ɵɵnextContext": runtime.nextContext,
            "ɵɵadvance": runtime.advance,
            "ɵɵtextInterpolate": runtime.interpolate,
        };
        runtime.template = function(
            index,
            template,
            declarations,
            bindings,
            tag,
            attributes,
            references,
            extractor
        ) {
            createTemplate(
                index,
                template,
                declarations,
                bindings,
                tag,
                attributes,
                references,
                extractor
            );
            return runtime.template;
        };
        runtime.defer = function(
            index,
            primary,
            dependencies,
            loading,
            placeholder,
            error,
            loadingConfig,
            placeholderConfig,
            timers,
            flags
        ) {
            markFeature("NgDefer");
            createDefer(
                index,
                primary,
                dependencies,
                loading,
                placeholder,
                error,
                loadingConfig,
                placeholderConfig,
                timers,
                flags
            );
        };
        runtime.idle = function(timeout) {
            scheduleIdle({ timeout });
        };

        function PrimaryTemplate(rf) {
            if (rf & 1) {
                runtime.start(0, "article");
                runtime.text(1, "Deferred");
                runtime.end();
            }
        }

        function PlaceholderTemplate(rf) {
            if (rf & 1) {
                runtime.start(0, "p");
                runtime.text(1, "Waiting");
                runtime.end();
            }
        }

        class RenamedDeferredComponent {
            static compiled = runtime.component({
                type: RenamedDeferredComponent,
                selectors: [["renamed-deferred-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        runtime.template(0, PrimaryTemplate, 2, 0)(
                            1,
                            PlaceholderTemplate,
                            2,
                            0
                        );
                        runtime.defer(2, 0, null, null, 1);
                        runtime.idle();
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("renamed deferred-view roles should be analyzed");
    let component = &report.components[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@defer (on idle) {"));
    assert!(component.source.contains("<article>Deferred</article>"));
    assert!(component.source.contains("@placeholder {"));
    assert!(component.source.contains("<p>Waiting</p>"));
    assert!(!component.source.contains("<ng-template"));
    assert_eq!(
        component.stats.runtime_calls_observed,
        component.stats.rendered_instruction_calls
    );
}

#[test]
fn infers_closure_renamed_repeater_role_family() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
            "ɵɵadvance": runtime.advance,
            "ɵɵtextInterpolate": runtime.interpolate,
        };
        runtime.createRepeater = function(
            index,
            template,
            declarations,
            bindings,
            tag,
            attributes,
            track,
            usesComponent,
            emptyTemplate,
            emptyDeclarations,
            emptyBindings,
            emptyTag,
            emptyAttributes
        ) {
            markFeature("NgControlFlow");
            declareTemplate(index + 1, template, declarations, bindings);
            if (emptyTemplate !== undefined) {
                declareTemplate(
                    index + 2,
                    emptyTemplate,
                    emptyDeclarations,
                    emptyBindings
                );
            }
        };
        runtime.updateRepeater = function(collection) {
            const previous = setActiveConsumer(null);
            const selected = runtimeState.selectedIndex;
            try {
                const view = readView(selected);
                const live = readLiveCollection(view);
                reconcile(live, collection);
                updateIndexes(live);
                updateEmptyBlock(view);
            } finally {
                setActiveConsumer(previous);
            }
        };
        runtime.trackIdentity = function(index, item) {
            return item;
        };

        function RowTemplate(rf, context) {
            if (rf & 1) {
                runtime.start(0, "span");
                runtime.text(1);
                runtime.end();
            }
            if (rf & 2) {
                rf = context.$implicit;
                runtime.advance(1);
                runtime.interpolate(rf.label);
            }
        }

        class RenamedRepeaterComponent {
            items = [];

            static compiled = runtime.component({
                type: RenamedRepeaterComponent,
                selectors: [["renamed-repeater-card"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        runtime.createRepeater(
                            0,
                            RowTemplate,
                            2,
                            0,
                            "span",
                            null,
                            runtime.trackIdentity
                        );
                    }
                    if (rf & 2) {
                        runtime.updateRepeater(context.items);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("renamed repeater roles should be analyzed");
    let component = &report.components[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains("@for (item of items; track item) {"));
    assert!(component.source.contains("<span>{{ item.label }}</span>"));
    assert_eq!(
        component.stats.runtime_calls_observed,
        component.stats.rendered_instruction_calls
    );
}

#[test]
fn rejects_a_repeater_create_lookalike_without_runtime_marker() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
        };
        runtime.lookalike = function(
            index,
            template,
            declarations,
            bindings,
            tag,
            attributes,
            track,
            usesComponent,
            emptyTemplate,
            emptyDeclarations,
            emptyBindings,
            emptyTag,
            emptyAttributes
        ) {
            unrelatedFeature("NotAngularControlFlow");
            declareSomething(index, template, declarations, bindings);
        };
        runtime.trackIdentity = function(index, item) {
            return item;
        };

        function RowTemplate(rf) {
            if (rf & 1) {
                runtime.start(0, "span");
                runtime.text(1, "Item");
                runtime.end();
            }
        }

        class RepeaterLookalikeComponent {
            static compiled = runtime.component({
                type: RepeaterLookalikeComponent,
                selectors: [["repeater-lookalike"]],
                template: function(rf) {
                    if (rf & 1) {
                        runtime.start(0, "section");
                        runtime.lookalike(
                            1,
                            RowTemplate,
                            2,
                            0,
                            "span",
                            null,
                            runtime.trackIdentity
                        );
                        runtime.end();
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the repeater lookalike should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component
        .issues
        .iter()
        .any(|issue| issue.kind == AngularRecoveryIssueKind::UnknownRuntimeInstruction));
    assert!(!component.source.contains("@for"));
}

#[test]
fn rejects_defer_views_that_are_not_trailing_siblings() {
    let source = r#"
        import * as core from "@angular/core";

        function PrimaryTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "article");
            }
        }

        class MalformedDeferredComponent {
            static compiled = core.ɵɵdefineComponent({
                type: MalformedDeferredComponent,
                selectors: [["malformed-deferred-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵtemplate(0, PrimaryTemplate, 1, 0);
                        core.ɵɵelement(1, "aside");
                        core.ɵɵdefer(2, 0, null);
                        core.ɵɵdeferOnIdle();
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("malformed deferred-view ordering should remain recoverable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MissingTargetNode
            && issue.instruction.as_deref() == Some("ɵɵdefer")
            && issue
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("trailing siblings"))
    }));
    assert!(!component.source.contains("@defer"));
    assert!(component.source.contains("<ng-template>"));
    assert!(component.source.contains("<aside></aside>"));
}

#[test]
fn infers_a_closure_specialized_one_parameter_conditional() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
            "ɵɵtemplate": runtime.template,
        };
        runtime.conditional = function(selectedIndex) {
            controlFlowMarker();
            const previousIndex =
                readBinding() !== noChange ? readBinding() : -1;
            if (bindingChanged(selectedIndex)) {
                destroyView(previousIndex);
                createView(selectedIndex);
                insertView(selectedIndex);
                attachView(selectedIndex);
            }
        };

        function StaticConditionalHost(rf, context) {
            if (rf & 1) {
                runtime.start(0, "div");
                runtime.end();
            }
            if (rf & 2) {
                runtime.conditional(
                    context.primary ? (context.secondary ? 0 : 1) : -1
                );
            }
        }

        function DetailsTemplate(rf) {
            if (rf & 1) {
                runtime.start(0, "p");
                runtime.text(1, "Details");
                runtime.end();
            }
        }

        function EmptyTemplate(rf) {
            if (rf & 1) {
                runtime.start(0, "p");
                runtime.text(1, "Empty");
                runtime.end();
            }
        }

        class ConditionalComponent {
            showDetails = true;

            static compiled = runtime.component({
                type: ConditionalComponent,
                selectors: [["conditional-card"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        runtime.start(0, "article");
                        runtime.template(1, DetailsTemplate, 2, 0, "p");
                        runtime.template(2, EmptyTemplate, 2, 0, "p");
                        runtime.end();
                    }
                    if (rf & 2) {
                        runtime.conditional(context.showDetails ? 1 : 2);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the specialized conditional should be analyzed");
    let component = &report.components[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@if (showDetails) {"));
    assert!(component.source.contains("<p>Details</p>"));
    assert!(component.source.contains("@else {"));
    assert!(component.source.contains("<p>Empty</p>"));
}

#[test]
fn rejects_an_embedded_template_continuation_without_shared_forwarding() {
    let source = r#"
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
        };
        runtime.firstTemplate = function(index, template, decls, vars) {
            createFirst(currentView(), index, template, decls, vars);
            return runtime.nextTemplate;
        };
        runtime.nextTemplate = function(
            index,
            template,
            decls,
            vars,
            tag,
            attrs,
            localRefs,
            extractor
        ) {
            createNext(
                currentView(),
                index,
                template,
                decls,
                vars,
                tag,
                attrs,
                localRefs,
                extractor
            );
            return runtime.nextTemplate;
        };

        function DetailsTemplate(rf) {
            if (rf & 1) {
                runtime.start(0, "p");
                runtime.text(1, "Details");
                runtime.end();
            }
        }

        class LookalikeComponent {
            static compiled = runtime.component({
                type: LookalikeComponent,
                selectors: [["lookalike-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        runtime.start(0, "article");
                        runtime.firstTemplate(1, DetailsTemplate, 2, 0);
                        runtime.end();
                    }
                },
            });
        }

        class EmptyWrapperComponent {
            static compiled = runtime.component({
                type: EmptyWrapperComponent,
                selectors: [["empty-wrapper-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        runtime.start(0, "article");
                        runtime.emptyFirstTemplate(1, DetailsTemplate, 2, 0);
                        runtime.end();
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the lookalike template continuation should be analyzed");
    for selector in ["lookalike-card", "empty-wrapper-card"] {
        let component = report
            .components
            .iter()
            .find(|component| component.selector == selector)
            .expect("both lookalike components should be reported");
        assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
        assert!(component
            .source
            .contains("Unsupported Ivy instruction: unknown-runtime-instruction"));
        assert!(!component.source.contains("<p>Details</p>"));
    }
}

#[test]
fn recovers_assignment_backed_nested_view_functions_through_a_stable_alias() {
    let source = r#"
        import * as core from "@angular/core";

        (function() {
            var AssignedDetailsTemplate, AssignedDetailsAlias;

            AssignedDetailsTemplate = function(rf) {
                if (rf & 1) {
                    core.ɵɵelementStart(0, "p");
                    core.ɵɵtext(1, "Assigned details");
                    core.ɵɵelementEnd();
                }
            };
            AssignedDetailsAlias = AssignedDetailsTemplate;

            class AssignmentBackedComponent {
                showDetails = true;

                static ɵcmp = core.ɵɵdefineComponent({
                    type: AssignmentBackedComponent,
                    selectors: [["assignment-backed"]],
                    template: function(rf, context) {
                        if (rf & 1) {
                            core.ɵɵtemplate(
                                0,
                                AssignedDetailsAlias,
                                2,
                                0,
                                "p"
                            );
                        }
                        if (rf & 2) {
                            core.ɵɵconditional(context.showDetails ? 0 : -1);
                        }
                    },
                });
            }
        })();
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("assignment-backed nested views should be analyzed");
    let component = &report.components[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@if (showDetails) {"));
    assert!(component.source.contains("<p>Assigned details</p>"));
    assert_eq!(
        component.stats.runtime_calls_observed,
        component.stats.rendered_instruction_calls
    );
    assert_eq!(component.stats.malformed_instruction_calls, 0);
}

#[test]
fn recovers_inverted_single_branch_conditional_selection() {
    let source = r#"
        import * as core from "@angular/core";

        function EmptyState(rf) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "p");
                core.ɵɵtext(1, "No results");
                core.ɵɵelementEnd();
            }
        }

        class InvertedConditionalComponent {
            hasResults = true;

            static ɵcmp = core.ɵɵdefineComponent({
                type: InvertedConditionalComponent,
                selectors: [["inverted-conditional"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵtemplate(0, EmptyState, 2, 0, "p");
                    }
                    if (rf & 2) {
                        core.ɵɵconditional(context.hasResults ? -1 : 0);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("inverted conditional fixture should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@if (!(hasResults)) {"));
    assert!(component.source.contains("<p>No results</p>"));
}

#[test]
fn recovers_nested_conditional_selection_in_logical_branch_order() {
    let source = r#"
        import * as core from "@angular/core";

        function DefaultView(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "p");
            }
        }
        function FirstView(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "h1");
            }
        }
        function SecondView(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "h2");
            }
        }
        function ThirdView(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "h3");
            }
        }

        class NestedConditionalComponent {
            outer = true;
            first = false;
            second = true;

            static ɵcmp = core.ɵɵdefineComponent({
                type: NestedConditionalComponent,
                selectors: [["nested-conditional"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵtemplate(0, DefaultView, 1, 0, "p");
                        core.ɵɵtemplate(1, FirstView, 1, 0, "h1");
                        core.ɵɵtemplate(2, SecondView, 1, 0, "h2");
                        core.ɵɵtemplate(3, ThirdView, 1, 0, "h3");
                    }
                    if (rf & 2) {
                        core.ɵɵconditional(
                            context.outer
                                ? context.first
                                    ? 1
                                    : context.second
                                      ? 2
                                      : 3
                                : 0
                        );
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("nested conditional fixture should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    let first = component
        .source
        .find("@if ((outer) && (first))")
        .expect("the first logical branch should render");
    let second = component
        .source
        .find("@else if ((outer) && !(first) && (second))")
        .expect("the second logical branch should render");
    let third = component
        .source
        .find("@else if ((outer) && !(first) && !(second))")
        .expect("the third logical branch should render");
    let fallback = component
        .source
        .find("@else {")
        .expect("the outer fallback should render");
    assert!(first < second && second < third && third < fallback);
}

#[test]
fn rejects_a_reassigned_assignment_backed_nested_view_function() {
    let source = r#"
        import * as core from "@angular/core";

        (function() {
            var ReassignedTemplate;

            ReassignedTemplate = function(rf) {
                if (rf & 1) {
                    core.ɵɵelement(0, "p");
                }
            };
            ReassignedTemplate = function(rf) {
                if (rf & 1) {
                    core.ɵɵelement(0, "section");
                }
            };

            class ReassignedViewComponent {
                static ɵcmp = core.ɵɵdefineComponent({
                    type: ReassignedViewComponent,
                    selectors: [["reassigned-view"]],
                    template: function(rf) {
                        if (rf & 1) {
                            core.ɵɵtemplate(
                                0,
                                ReassignedTemplate,
                                1,
                                0,
                                "section"
                            );
                        }
                    },
                });
            }
        })();
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the component should remain recoverable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MalformedInstruction
            && issue.instruction.as_deref() == Some("ɵɵtemplate")
            && issue.detail.as_deref() == Some("embedded template function could not be resolved")
    }));
    assert_eq!(component.stats.runtime_calls_observed, 1);
    assert_eq!(component.stats.rendered_instruction_calls, 0);
    assert_eq!(component.stats.malformed_instruction_calls, 1);
}

#[test]
fn recovers_projection_local_references_and_pipe_bindings() {
    let source = r#"
        import * as core from "@angular/core";

        class BindingComponent {
            title = "Bindings";

            static ɵcmp = core.ɵɵdefineComponent({
                type: BindingComponent,
                selectors: [["binding-card"]],
                ngContentSelectors: ["[extra]"],
                consts: [["selectButton", ""]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵprojectionDef([[["", "extra", ""]]]);
                        core.ɵɵelementStart(0, "article");
                        core.ɵɵelementStart(1, "h2");
                        core.ɵɵtext(2);
                        core.ɵɵpipe(3, "uppercase");
                        core.ɵɵelementEnd();
                        core.ɵɵelementStart(4, "button", null, 0);
                        core.ɵɵtext(6, "Select");
                        core.ɵɵelementEnd();
                        core.ɵɵelementStart(7, "small");
                        core.ɵɵtext(8);
                        core.ɵɵelementEnd();
                        core.ɵɵprojection(9);
                        core.ɵɵelementEnd();
                    }
                    if (rf & 2) {
                        const selectButton = core.ɵɵreference(5);
                        core.ɵɵadvance(2);
                        core.ɵɵtextInterpolate(
                            core.ɵɵpipeBind1(3, 0, context.title)
                        );
                        core.ɵɵadvance(6);
                        core.ɵɵtextInterpolate1(
                            "Disabled: ",
                            selectButton.disabled
                        );
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("projection, references, and pipes should be analyzed");

    assert_eq!(report.components.len(), 1);
    let component = &report.components[0];
    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains("<h2>{{ title | uppercase }}</h2>"));
    assert!(component
        .source
        .contains("<button #selectButton>Select</button>"));
    assert!(component
        .source
        .contains("<small>Disabled: {{ selectButton.disabled }}</small>"));
    assert!(component
        .source
        .contains(r#"<ng-content select="[extra]" />"#));
    assert_eq!(component.stats.runtime_calls_observed, 20);
    assert_eq!(component.stats.rendered_instruction_calls, 20);
    assert_eq!(component.stats.unsupported_runtime_calls, 0);
    assert_eq!(component.stats.malformed_instruction_calls, 0);
}

#[test]
fn recovers_repeater_views_with_assignment_backed_track_and_restored_listeners() {
    let source = r#"
        import * as core from "@angular/core";

        let trackItem;
        trackItem = ($index, $item) => $item.id;

        function RowTemplate(rf, context) {
            if (rf & 1) {
                const savedView = core.ɵɵgetCurrentView();
                core.ɵɵelementStart(0, "button", 2, 0);
                core.ɵɵlistener("click", function() {
                    const item_r1 = core.ɵɵrestoreView(savedView).V;
                    const row_r2 = core.ɵɵreference(1);
                    core.ɵɵnextContext().select(row_r2, item_r1);
                    return core.ɵɵresetView();
                });
                core.ɵɵtext(2);
                core.ɵɵelementEnd();
            }
            if (rf & 2) {
                const item_r1 = context.V;
                core.ɵɵadvance(2);
                core.ɵɵtextInterpolate1(" ", item_r1.label, " ");
            }
        }

        function EmptyTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "p");
                core.ɵɵtext(1, "No items");
                core.ɵɵelementEnd();
            }
        }

        class RepeaterComponent {
            items = [];
            select(row, item) {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: RepeaterComponent,
                selectors: [["repeater-card"]],
                consts: [["row", ""], ["type", "button"], ["type", "button", 3, "click"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵrepeaterCreate(
                            0,
                            RowTemplate,
                            3,
                            1,
                            "button",
                            1,
                            trackItem,
                            false,
                            EmptyTemplate,
                            2,
                            0,
                            "p"
                        );
                    }
                    if (rf & 2) {
                        core.ɵɵrepeater(context.items);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("repeater views should be analyzed");
    let component = &report.components[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains("@for (item of items; track item.id) {"));
    assert!(component
        .source
        .contains(r#"<button type="button" #row (click)="select(row, item)">"#));
    assert!(component.source.contains("{{ item.label }}"));
    assert!(component.source.contains("@empty {"));
    assert!(component.source.contains("<p>No items</p>"));
    assert!(!component.source.contains("ɵɵ"));
    assert_eq!(component.stats.runtime_calls_observed, 16);
    assert_eq!(component.stats.rendered_instruction_calls, 16);
    assert_eq!(component.stats.unsupported_runtime_calls, 0);
    assert_eq!(component.stats.malformed_instruction_calls, 0);
}

#[test]
fn recovers_nested_view_aliases_and_sequence_wrapped_listener_actions() {
    let source = r#"
        import * as core from "@angular/core";

        const trackItem = ($index, item) => item.id;
        const runtimeState = { view: [] };

        function ConditionalView(rf) {
            if (rf & 1) {
                const savedView = core.ɵɵgetCurrentView();
                core.ɵɵelementStart(0, "button", 2, 0);
                core.ɵɵlistener("click", function() {
                    core.ɵɵrestoreView(savedView);
                    const button = runtimeState.view[28];
                    const item = core.ɵɵnextContext().$implicit;
                    const component = core.ɵɵnextContext();
                    const displayLabel = core.ɵɵreadContextLet(0);
                    return (
                        component.record(button, item, displayLabel),
                        core.ɵɵresetView(component.active = false)
                    );
                });
                core.ɵɵtext(2);
                core.ɵɵelementEnd();
            }
            if (rf & 2) {
                const item = core.ɵɵnextContext().$implicit;
                core.ɵɵadvance(2);
                core.ɵɵtextInterpolate(item.label);
            }
        }

        function RepeaterView(rf) {
            if (rf & 1) {
                core.ɵɵtemplate(0, ConditionalView, 3, 1, "button", 1);
            }
            if (rf & 2) {
                const component = core.ɵɵnextContext();
                core.ɵɵconditional(component.active ? 0 : -1);
            }
        }

        class NestedListenerComponent {
            active = true;
            items = [];
            prefix = "Selected: ";
            suffix = "item";
            record(button, item, displayLabel) {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: NestedListenerComponent,
                selectors: [["nested-listener"]],
                consts: [
                    ["button", ""],
                    ["type", "button"],
                    ["type", "button", 3, "click"],
                ],
                template: function(rf, component) {
                    if (rf & 1) {
                        core.ɵɵdeclareLet(0);
                        core.ɵɵrepeaterCreate(
                            1,
                            RepeaterView,
                            1,
                            1,
                            null,
                            null,
                            trackItem
                        );
                    }
                    if (rf & 2) {
                        core.ɵɵstoreLet(component.prefix + component.suffix);
                        core.ɵɵadvance();
                        core.ɵɵrepeater(component.items);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("nested view aliases should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains("@let displayLabel = prefix + suffix;"));
    assert!(component
        .source
        .contains("@for (item of items; track item.id) {"));
    assert!(component.source.contains("@if (active) {"));
    assert!(component
        .source
        .contains("(click)=\"record(button, item, displayLabel); active = false\""));
    assert!(component.source.contains("{{ item.label }}"));
    assert!(!component.source.contains("$implicit"));
}

#[test]
fn recovers_optional_chain_temps_in_restored_listeners() {
    let source = r#"
        import * as core from "@angular/core";

        class OptionalListenerComponent {
            input() {}
            activate() {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: OptionalListenerComponent,
                selectors: [["optional-listener"]],
                consts: [
                    ["type", "button", 3, "click"],
                ],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(0, "button", 0);
                        core.ɵɵlistener("click", function($event) {
                            const component = core.ɵɵrestoreView(savedView);
                            let temporary;
                            $event.target !== (
                                (temporary = component.input()) == null
                                    ? void 0
                                    : temporary.nativeElement
                            ) && component.activate();
                            return core.ɵɵresetView();
                        });
                        core.ɵɵtext(1, "Activate");
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("optional-chain listener temp should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(
        component.source.contains(
            r#"(click)="$event.target !== (input()?.nativeElement) &amp;&amp; activate()""#
        ),
        "{}",
        component.source
    );
    assert!(!component.source.contains("temporary"));
}

#[test]
fn synthesizes_a_class_method_for_structured_restored_listener_statements() {
    let source = r#"
        import * as core from "@angular/core";

        const finalize = (value) => value;

        class StructuredListenerComponent {
            current() {}
            prepare(value) {}
            commit(value) {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: StructuredListenerComponent,
                selectors: [["structured-listener"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(0, "button");
                        core.ɵɵlistener("click", function() {
                            const component = core.ɵɵrestoreView(savedView);
                            const result = component.current();
                            if (result) {
                                let temporary;
                                temporary = component.prepare(result);
                                component.commit(finalize(temporary));
                            }
                            return core.ɵɵresetView();
                        });
                        core.ɵɵtext(1, "Commit");
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("structured restored listener should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"(click)="recoveredClick()""#));
    assert!(component.source.contains("recoveredClick() {"));
    assert!(component.source.contains("const result = this.current();"));
    assert!(component.source.contains("if (result) {"));
    assert!(component.source.contains("let temporary;"));
    assert!(component
        .source
        .contains("temporary = this.prepare(result);"));
    assert!(component
        .source
        .contains("this.commit(finalize(temporary));"));
    assert!(component
        .source
        .contains("const finalize = (value)=>value;"));
    assert!(!component.source.contains("ɵɵrestoreView"));
    assert!(!component.source.contains("ɵɵresetView"));
    assert_typescript_parses(&component.source);
}

#[test]
fn passes_event_and_view_locals_to_a_synthesized_listener_method() {
    let source = r#"
        import * as core from "@angular/core";

        const trackItem = ($index, item) => item.id;

        function RowTemplate(renderFlags, context) {
            if (renderFlags & 1) {
                const savedView = core.ɵɵgetCurrentView();
                core.ɵɵelementStart(0, "button");
                core.ɵɵlistener("click", function(event) {
                    const item = core.ɵɵrestoreView(savedView).$implicit;
                    const component = core.ɵɵnextContext();
                    event = component.normalize(event);
                    const accepted = component.prepare(item, event);
                    if (accepted) {
                        component.commit(item);
                    }
                    return core.ɵɵresetView(accepted);
                });
                core.ɵɵtext(1);
                core.ɵɵelementEnd();
            }
            if (renderFlags & 2) {
                const item = context.$implicit;
                core.ɵɵadvance();
                core.ɵɵtextInterpolate(item.label);
            }
        }

        class ScopedListenerComponent {
            items = [];
            normalize(event) {}
            prepare(item, event) {}
            commit(item) {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: ScopedListenerComponent,
                selectors: [["scoped-listener"]],
                template: function(renderFlags, component) {
                    if (renderFlags & 1) {
                        core.ɵɵrepeaterCreate(
                            0,
                            RowTemplate,
                            2,
                            1,
                            "button",
                            null,
                            trackItem
                        );
                    }
                    if (renderFlags & 2) {
                        core.ɵɵrepeater(component.items);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("listener method parameters should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains(r#"(click)="recoveredClick($event, item)""#));
    assert!(component.source.contains("recoveredClick($event, item) {"));
    assert!(component
        .source
        .contains("$event = this.normalize($event);"));
    assert!(component
        .source
        .contains("const accepted = this.prepare(item, $event);"));
    assert!(component.source.contains("this.commit(item);"));
    assert!(component.source.contains("return accepted;"));
    assert_typescript_parses(&component.source);
}

#[test]
fn preserves_shadowed_listener_bindings_and_avoids_method_name_collisions() {
    let source = r#"
        import * as core from "@angular/core";

        class ShadowedListenerComponent {
            current() {}
            recoveredClick() {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: ShadowedListenerComponent,
                selectors: [["shadowed-listener"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(0, "button");
                        core.ɵɵlistener("click", function() {
                            const component = core.ɵɵrestoreView(savedView);
                            const result = component.current();
                            if (result) {
                                const component = {
                                    commit() {},
                                };
                                component.commit();
                            }
                            return core.ɵɵresetView();
                        });
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("shadowed listener bindings should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"(click)="recoveredClick2()""#));
    assert!(component.source.contains("recoveredClick2() {"));
    assert!(component.source.contains("const result = this.current();"));
    assert!(component.source.contains("const component = {"));
    assert!(component.source.contains("component.commit();"));
    assert!(!component.source.contains("this.commit();"));
    assert_typescript_parses(&component.source);
}

#[test]
fn materializes_a_reassigned_ivy_alias_as_a_method_local() {
    let source = r#"
        import * as core from "@angular/core";

        class ReassignedAliasComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: ReassignedAliasComponent,
                selectors: [["reassigned-alias"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(0, "button");
                        core.ɵɵlistener("click", function() {
                            let local = core.ɵɵrestoreView(savedView);
                            local = {
                                commit(value) {
                                    console.log(value);
                                },
                            };
                            local.commit("value");
                            return core.ɵɵresetView();
                        });
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("reassigned Ivy aliases should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"(click)="recoveredClick()""#));
    assert!(component.source.contains("let local = this;"));
    assert!(component.source.contains("local = {"));
    assert!(component.source.contains("local.commit(\"value\");"));
    assert!(!component.source.contains("this.commit(\"value\");"));
    assert_typescript_parses(&component.source);
}

#[test]
fn rejects_control_flow_outside_the_structured_listener_subset() {
    let source = r#"
        import * as core from "@angular/core";

        class LoopListenerComponent {
            items = [];
            select(item) {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: LoopListenerComponent,
                selectors: [["loop-listener"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(0, "button");
                        core.ɵɵlistener("click", function() {
                            const component = core.ɵɵrestoreView(savedView);
                            const items = component.items;
                            for (const item of items) {
                                component.select(item);
                            }
                            return core.ɵɵresetView();
                        });
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("unsupported listener control flow should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MalformedInstruction
            && issue.instruction.as_deref() == Some("ɵɵlistener")
            && issue.detail.as_deref().is_some_and(|detail| {
                detail.contains("unsupported structured listener statement: for-of")
            })
    }));
    assert!(!component.source.contains("recoveredClick"));
}

#[test]
fn recovers_structural_i18n_regions_with_nested_elements() {
    let source = r#"
        import * as core from "@angular/core";

        function localize(message) {
            return message;
        }

        class StructuralI18nComponent {
            name = "reader";

            static ɵcmp = core.ɵɵdefineComponent({
                type: StructuralI18nComponent,
                selectors: [["structural-i18n"]],
                consts: () => [
                    localize("Hello \uFFFD#2\uFFFD\uFFFD0\uFFFD\uFFFD/#2\uFFFD!")
                ],
                template: function(rf, component) {
                    if (rf & 1) {
                        core.ɵɵelementStart(0, "p");
                        core.ɵɵi18nStart(1, 0);
                        core.ɵɵelement(2, "strong");
                        core.ɵɵi18nEnd();
                        core.ɵɵelementEnd();
                    }
                    if (rf & 2) {
                        core.ɵɵadvance(2);
                        core.ɵɵi18nExp(component.name);
                        core.ɵɵi18nApply(1);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("structural i18n should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("<p i18n>"));
    assert!(component
        .source
        .contains("Hello <strong>{{ name }}</strong>!"));
    assert!(!component.source.contains("ɵɵi18n"));
}

#[test]
fn recovers_projection_fallback_template_functions() {
    let source = r#"
        import * as core from "@angular/core";

        function TitleFallback(rf) {
            if (rf & 1) {
                core.ɵɵelementStart(0, "h2");
                core.ɵɵtext(1, "Fallback title");
                core.ɵɵelementEnd();
            }
        }

        class ProjectionFallbackComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: ProjectionFallbackComponent,
                selectors: [["projection-fallback"]],
                ngContentSelectors: ["[card-title]"],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵprojectionDef([[["", "card-title", ""]]]);
                        core.ɵɵprojection(
                            0,
                            0,
                            null,
                            TitleFallback,
                            2,
                            0
                        );
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("projection fallback should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains("<ng-content select=\"[card-title]\">"));
    assert!(component.source.contains("<h2>Fallback title</h2>"));
    assert!(component.source.contains("</ng-content>"));
}

#[test]
fn recovers_two_way_binding_instruction_triplets() {
    let source = r#"
        import * as core from "@angular/core";

        class TwoWayComponent {
            name = "reader";

            static ɵcmp = core.ɵɵdefineComponent({
                type: TwoWayComponent,
                selectors: [["two-way-binding"]],
                template: function(rf, component) {
                    if (rf & 1) {
                        core.ɵɵelementStart(0, "fixture-model-target");
                        core.ɵɵtwoWayListener("valueChange", function($event) {
                            return (
                                core.ɵɵtwoWayBindingSet(component.name, $event) ||
                                    (component.name = $event),
                                $event
                            );
                        });
                        core.ɵɵelementEnd();
                    }
                    if (rf & 2) {
                        core.ɵɵtwoWayProperty("value", component.name);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("two-way binding instructions should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"[(value)]="name""#));
    assert!(!component.source.contains("valueChange"));
    assert!(!component.source.contains("ɵɵtwoWay"));
}

#[test]
fn recovers_two_way_binding_through_a_restored_local_reference() {
    let source = r#"
        import * as core from "@angular/core";

        class RestoredTwoWayComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: RestoredTwoWayComponent,
                selectors: [["restored-two-way-binding"]],
                consts: [["target", ""]],
                template: function(rf) {
                    if (rf & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(
                            0,
                            "fixture-model-target",
                            null,
                            0
                        );
                        core.ɵɵtwoWayListener("valueChange", function($event) {
                            core.ɵɵrestoreView(savedView);
                            const target = core.ɵɵreference(1);
                            core.ɵɵtwoWayBindingSet(target.value, $event) ||
                                (target.value = $event);
                            return core.ɵɵresetView($event);
                        });
                        core.ɵɵelementEnd();
                    }
                    if (rf & 2) {
                        const target = core.ɵɵreference(1);
                        core.ɵɵtwoWayProperty("value", target.value);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("restored-view two-way binding instructions should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("#target"));
    assert!(component.source.contains(r#"[(value)]="target.value""#));
    assert!(!component.source.contains("ɵɵtwoWay"));
}

#[test]
fn recovers_two_way_binding_through_a_restored_parent_context() {
    let source = r#"
        import * as core from "@angular/core";

        class RestoredParentTwoWayComponent {
            name = "reader";

            static ɵcmp = core.ɵɵdefineComponent({
                type: RestoredParentTwoWayComponent,
                selectors: [["restored-parent-two-way-binding"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(0, "fixture-model-target");
                        core.ɵɵtwoWayListener("valueChange", function($event) {
                            core.ɵɵrestoreView(savedView);
                            const parent = core.ɵɵnextContext(2);
                            core.ɵɵtwoWayBindingSet(parent.name, $event) ||
                                (parent.name = $event);
                            return core.ɵɵresetView($event);
                        });
                        core.ɵɵelementEnd();
                    }
                    if (rf & 2) {
                        core.ɵɵtwoWayProperty("value", context.name);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("parent-context two-way binding instructions should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"[(value)]="name""#));
    assert!(!component.source.contains("ɵɵtwoWay"));
}

#[test]
fn recovers_animation_binding_and_listener_instructions() {
    let source = r#"
        import * as core from "@angular/core";

        class AnimationComponent {
            leaveClass = "fade-out";
            started(event) {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: AnimationComponent,
                selectors: [["animation-bindings"]],
                template: function(rf, component) {
                    if (rf & 1) {
                        core.ɵɵelementStart(0, "div");
                        core.ɵɵanimateLeave(function() {
                            return component.leaveClass;
                        });
                        core.ɵɵanimateEnter("fade-in");
                        core.ɵɵanimateEnterListener(function($event) {
                            component.started($event);
                        });
                        core.ɵɵtext(1, "Animated");
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("animation binding instructions should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"animate.enter="fade-in""#));
    assert!(component.source.contains(r#"[animate.leave]="leaveClass""#));
    assert!(component
        .source
        .contains(r#"(animate.enter)="started($event)""#));
    assert!(!component.source.contains("ɵɵanimate"));
}

#[test]
fn infers_closure_renamed_two_way_and_animation_families() {
    let source = r#"
        runtime.component = function(definition) {
            return Object.assign({}, definition);
        };
        runtime.elementStart = function(index, name, attrs, refs) {
            createElement(index, name, attrs, refs);
            return runtime.elementStart;
        };
        runtime.elementEnd = function() {
            closeElement();
            return runtime.elementEnd;
        };
        runtime.text = function(index, value = "") {
            createText(index, value);
        };

        runtime.listen = function(name, handler) {
            installOutput(name, handler);
            return runtime.listen;
        };
        runtime.bind = function(name, value, sanitizer) {
            isSignal(value) && typeof value.set == "function" && (value = value());
            const view = currentView();
            if (bindingChanged(view, value)) {
                writeProperty(currentNode(), view, name, value, currentRenderer(), sanitizer);
            }
            return runtime.bind;
        };
        runtime.set = function(target, value) {
            const writable = isSignal(target) && typeof target.set == "function";
            return writable && target.set(value), writable;
        };

        runtime.enter = function(value) {
            marker("NgAnimateEnter");
            schedule(() => normalizeAnimation(value));
            return runtime.enter;
        };
        runtime.enterListener = function(listener) {
            marker("NgAnimateEnter");
            schedule(() => listener.call(currentContext(), currentEvent()));
            return runtime.enterListener;
        };
        runtime.leave = function(value) {
            marker("NgAnimateLeave");
            schedule(() => normalizeAnimation(value));
            return runtime.leave;
        };

        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.elementStart,
            "ɵɵelementEnd": runtime.elementEnd,
            "ɵɵtext": runtime.text,
        };

        class RenamedBindingFamiliesComponent {
            name = "reader";
            leaveClass = "fade-out";
            started() {}

            static compiled = runtime.component({
                type: RenamedBindingFamiliesComponent,
                selectors: [["renamed-binding-families"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.elementStart(0, "fixture-model-target");
                        runtime.listen("valueChange", function(event) {
                            return (
                                runtime.set(context.name, event) ||
                                    (context.name = event),
                                event
                            );
                        });
                        runtime.elementEnd();
                        runtime.elementStart(1, "div");
                        runtime.leave(function() {
                            return context.leaveClass;
                        });
                        runtime.enter("fade-in");
                        runtime.enterListener(function() {
                            context.started();
                        });
                        runtime.text(2, "Animated");
                        runtime.elementEnd();
                    }
                    if (renderFlags & 2) {
                        runtime.bind("value", context.name);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-renamed Angular 22 binding families should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"[(value)]="name""#));
    assert!(component.source.contains(r#"[animate.leave]="leaveClass""#));
    assert!(component.source.contains(r#"animate.enter="fade-in""#));
    assert!(component.source.contains(r#"(animate.enter)="started()""#));
}

#[test]
fn resolves_reference_aliases_at_their_view_context_depth() {
    let source = r#"
        import * as core from "@angular/core";

        function ChildView(rf) {
            if (rf & 1) {
                const savedView = core.ɵɵgetCurrentView();
                core.ɵɵelementStart(0, "span");
                core.ɵɵlistener("click", function() {
                    core.ɵɵrestoreView(savedView);
                    core.ɵɵnextContext();
                    const parentButton = core.ɵɵreference(1);
                    parentButton.focus();
                    return core.ɵɵresetView();
                });
                core.ɵɵtext(1, "Child");
                core.ɵɵelementEnd();
            }
            if (rf & 2) {
                core.ɵɵnextContext();
                const parentButton = core.ɵɵreference(1);
                core.ɵɵproperty("title", parentButton.title);
            }
        }

        function MissingAncestorView(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "span");
            }
            if (rf & 2) {
                core.ɵɵnextContext(2);
                const missing = core.ɵɵreference(1);
                core.ɵɵproperty("title", missing.title);
            }
        }

        class ParentReferenceComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: ParentReferenceComponent,
                selectors: [["parent-reference"]],
                consts: [["parentButton", ""]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "button", null, 0);
                        core.ɵɵtemplate(2, ChildView, 2, 1, "span");
                    }
                    if (rf & 2) {
                        core.ɵɵconditional(context.show ? 2 : -1);
                    }
                },
            });
        }

        class MissingAncestorComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: MissingAncestorComponent,
                selectors: [["missing-ancestor"]],
                consts: [["parentButton", ""]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "button", null, 0);
                        core.ɵɵtemplate(2, MissingAncestorView, 1, 1, "span");
                    }
                    if (rf & 2) {
                        core.ɵɵconditional(context.show ? 2 : -1);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("view-scoped references should be analyzed");
    let parent = report
        .components
        .iter()
        .find(|component| component.selector == "parent-reference")
        .expect("the parent-reference component should recover");
    let missing = report
        .components
        .iter()
        .find(|component| component.selector == "missing-ancestor")
        .expect("the missing-ancestor component should remain visible");

    assert_eq!(
        parent.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        parent.issues,
        parent.source,
    );
    assert!(parent.source.contains("<button #parentButton></button>"));
    assert!(parent.source.contains(r#"(click)="parentButton.focus()""#));
    assert!(parent.source.contains(r#"[title]="parentButton.title""#));
    assert_eq!(missing.completeness, AngularRecoveryCompleteness::Partial);
    assert!(missing.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MissingTargetNode
            && issue.instruction.as_deref() == Some("ɵɵreference")
            && issue
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("context depth 2"))
    }));
}

#[test]
fn resolves_later_parent_references_from_an_embedded_view() {
    let source = r#"
        import * as core from "@angular/core";

        function EarlyChildView(rf) {
            if (rf & 1) {
                const savedView = core.ɵɵgetCurrentView();
                core.ɵɵelementStart(0, "button");
                core.ɵɵlistener("click", function() {
                    core.ɵɵrestoreView(savedView);
                    const parent = core.ɵɵnextContext();
                    const laterInput = core.ɵɵreference(2);
                    return core.ɵɵresetView(parent.focusInput(laterInput));
                });
                core.ɵɵtext(1, "Focus later input");
                core.ɵɵelementEnd();
            }
        }

        class ForwardReferenceComponent {
            show = true;
            focusInput(input) {}

            static ɵcmp = core.ɵɵdefineComponent({
                type: ForwardReferenceComponent,
                selectors: [["forward-reference"]],
                constantPoolFactory: () => [
                    ["laterInput", ""],
                    ["class", "later"],
                ],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵtemplate(0, EarlyChildView, 2, 0, "button");
                        core.ɵɵelement(1, "input", 1, 0);
                    }
                    if (rf & 2) {
                        core.ɵɵconditional(context.show ? 0 : -1);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("a later parent reference should remain visible to an earlier child view");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@if (show) {"));
    assert!(component
        .source
        .contains(r#"(click)="focusInput(laterInput)""#));
    assert!(component
        .source
        .contains(r#"<input class="later" #laterInput />"#));
}

#[test]
fn decodes_a_mixed_direct_constant_table_with_opaque_entries() {
    let source = r#"
        import * as core from "@angular/core";

        class MixedConstantTableComponent {
            static compiled = core.ɵɵdefineComponent({
                type: MixedConstantTableComponent,
                selectors: [["mixed-constant-table"]],
                constantPool: [
                    ["trigger", ""],
                    "viewBox;0 0 24 24".split(";"),
                    [1, "panel", 3, "click"],
                ],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        core.ɵɵelement(0, "button", 2, 0);
                        core.ɵɵelement(2, "svg", 1);
                    }
                    if (renderFlags & 2) {
                        const trigger = core.ɵɵreference(1);
                        core.ɵɵproperty("aria-label", trigger.label);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("mixed direct constant tables should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains(r#"<button class="panel" #trigger [aria-label]="trigger.label"></button>"#));
    assert!(component
        .source
        .contains(r#"<svg viewBox="0 0 24 24"></svg>"#));
}

#[test]
fn infers_closure_renamed_namespace_switches() {
    let source = r#"
        import * as core from "@angular/core";

        _.svg = function() {
            _.state.namespace = "svg";
        };
        _.math = function() {
            _.state.namespace = "math";
        };
        _.html = function() {
            _.state.namespace = null;
        };

        class NamespaceComponent {
            static compiled = core.ɵɵdefineComponent({
                type: NamespaceComponent,
                selectors: [["namespace-fixture"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        _.svg();
                        core.ɵɵelementStart(0, "svg");
                        core.ɵɵelement(1, "circle");
                        core.ɵɵelementEnd();
                        _.math();
                        core.ɵɵelementStart(2, "math");
                        core.ɵɵelementStart(3, "mi");
                        core.ɵɵtext(4, "x");
                        core.ɵɵelementEnd();
                        core.ɵɵelementEnd();
                        _.html();
                        core.ɵɵelementStart(5, "p");
                        core.ɵɵtext(6, "HTML");
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-renamed namespace helpers should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("<svg>"));
    assert!(component.source.contains("<circle></circle>"));
    assert!(component.source.contains("<math>"));
    assert!(component.source.contains("<mi>x</mi>"));
    assert!(component.source.contains("<p>HTML</p>"));
}

#[test]
fn infers_closure_view_state_helpers_and_an_inlined_current_view_capture() {
    let source = r#"
        runtime.define = function(definition) { return definition; };
        runtime.start = function(index, name) {
            createNode(index, name);
            return runtime.start;
        };
        runtime.end = function() {
            leaveNode();
            return runtime.end;
        };
        runtime.text = function(index, value = "") {
            createText(index, value);
        };
        runtime.listener = function(name, handler, target) {
            addListener(name, handler, target);
            return runtime.listener;
        };
        runtime.template = function(index, view, decls, vars, tag) {
            createTemplate(index, view, decls, vars, tag);
            return runtime.template;
        };
        runtime.conditional = function(index) {
            selectTemplate(index);
        };
        runtime.property = function(name, value, sanitizer) {
            setProperty(name, value, sanitizer);
            return runtime.property;
        };
        runtime.reference = function(slot) {
            return runtime.state.context[27 + slot];
        };
        runtime.checkedReference = function(slot) {
            slot = runtime.state.context[27 + slot];
            if (slot === runtime.noChange) {
                throw new Error("uninitialized local reference");
            }
            return slot;
        };
        runtime.next = function(depth = 1) {
            let view = runtime.state.context;
            while (depth > 0) {
                view = view[14];
                depth--;
            }
            return (runtime.state.context = view)[8];
        };
        runtime.get = function() {
            return runtime.state.current;
        };
        runtime.restore = function(view) {
            return runtime.state.context = view, view[8];
        };
        runtime.reset = function(value) {
            return runtime.state.context = null, value;
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
            "ɵɵlistener": runtime.listener,
            "ɵɵtemplate": runtime.template,
            "ɵɵconditional": runtime.conditional,
            "ɵɵproperty": runtime.property,
        };

        function ConditionalButton(rf) {
            if (rf & 1) {
                const savedView = runtime.state.current;
                const savedViewFromGetter = runtime.get();
                runtime.start(0, "button", null, 0);
                runtime.listener("click", function(event) {
                    runtime.restore(savedView);
                    event.preventDefault();
                    const action = runtime.checkedReference(1);
                    const actions = runtime.next().actions;
                    return runtime.reset(actions.select(action));
                });
                runtime.text(1, "Nested view");
                runtime.end();
                runtime.start(2, "button");
                runtime.listener("click", function() {
                    runtime.restore(savedViewFromGetter);
                    const action = runtime.reference(1);
                    const context = runtime.next();
                    context.select(action);
                    return runtime.reset();
                });
                runtime.text(3, "Named capture");
                runtime.end();
            }
            if (rf & 2) {
                const checkedAction = runtime.checkedReference(1);
                const directAction = runtime.reference(1);
                const context = runtime.next();
                runtime.property("title", directAction.title);
                runtime.property("disabled", context.disabled);
            }
        }

        class ClosureViewStateComponent {
            static compiled = runtime.define({
                type: ClosureViewStateComponent,
                selectors: [["closure-view-state"]],
                consts: [["action", ""]],
                template: function(rf, context) {
                    if (rf & 1) {
                        runtime.template(0, ConditionalButton, 4, 1, "button");
                    }
                    if (rf & 2) {
                        runtime.conditional(context.visible ? 0 : -1);
                    }
                },
            });
        }

        class ContextEffectComponent {
            static compiled = runtime.define({
                type: ContextEffectComponent,
                selectors: [["context-effect"]],
                template: function(rf) {
                    if (rf & 1) {
                        runtime.start(0, "div");
                        runtime.end();
                    }
                    if (rf & 2) {
                        runtime.next();
                    }
                },
            });
        }
        void runtime.public;
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure view-state helpers should be analyzed");
    let component = report
        .components
        .iter()
        .find(|component| component.selector == "closure-view-state")
        .expect("the captured-view component should recover");
    let effect = report
        .components
        .iter()
        .find(|component| component.selector == "context-effect")
        .expect("the effect-only context component should remain visible");

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@if (visible) {"));
    assert!(component.source.contains(
        r#"<button #action (click)="$event.preventDefault(); actions.select(action)" [title]="action.title" [disabled]="disabled">Nested view</button>"#
    ));
    assert!(component
        .source
        .contains(r#"<button (click)="select(action)">Named capture</button>"#));
    assert!(!component.source.contains("runtime."));
    assert_eq!(component.stats.unsupported_runtime_calls, 0);
    assert_eq!(component.stats.malformed_instruction_calls, 0);
    assert_eq!(
        effect.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        effect.issues,
        effect.source,
    );
}

#[test]
fn rejects_unpaired_closure_view_state_lookalikes() {
    let source = r#"
        runtime.define = function(definition) { return definition; };
        runtime.start = function(index, name) {
            createNode(index, name);
            return runtime.start;
        };
        runtime.end = function() {
            leaveNode();
            return runtime.end;
        };
        runtime.listener = function(name, handler, target) {
            addListener(name, handler, target);
            return runtime.listener;
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵlistener": runtime.listener,
        };
        runtime.restoreLookalike = function(view) {
            return runtime.state.context = view, view[8];
        };
        runtime.resetLookalike = function(value) {
            return runtime.other.context = null, value;
        };

        class ViewStateLookalikeComponent {
            static compiled = runtime.define({
                type: ViewStateLookalikeComponent,
                selectors: [["view-state-lookalike"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        const savedView = runtime.state.current;
                        runtime.start(0, "button");
                        runtime.listener("click", function() {
                            runtime.restoreLookalike(savedView);
                            return runtime.resetLookalike(context.select());
                        });
                        runtime.end();
                    }
                },
            });
        }
        void runtime.public;
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("unpaired view-state lookalikes should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(!component.source.contains(r#"(click)="select()""#));
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::UnsupportedStatement
            && issue.detail.as_deref() == Some("declaration")
    }));
}

#[test]
fn rejects_unchecked_closure_slot_loads_as_references() {
    let source = r#"
        runtime.define = function(definition) { return definition; };
        runtime.start = function(index, name) {
            createNode(index, name);
            return runtime.start;
        };
        runtime.end = function() {
            leaveNode();
            return runtime.end;
        };
        runtime.property = function(name, value, sanitizer) {
            setProperty(name, value, sanitizer);
            return runtime.property;
        };
        runtime.slotLoad = function(slot) {
            return runtime.state.context[27 + slot];
        };
        runtime.referenceLookalike = function(slot) {
            return runtime.state.context[28 + slot];
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵproperty": runtime.property,
        };

        class ReferenceLookalikeComponent {
            static compiled = runtime.define({
                type: ReferenceLookalikeComponent,
                selectors: [["reference-lookalike"]],
                template: function(rf) {
                    if (rf & 1) {
                        runtime.start(0, "button");
                        runtime.end();
                    }
                    if (rf & 2) {
                        const unchecked = runtime.slotLoad(1);
                        const target = runtime.referenceLookalike(1);
                        runtime.property("disabled", target.disabled);
                    }
                },
            });
        }
        void runtime.public;
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("a reference lookalike should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component
        .source
        .contains("Unsupported Ivy instruction: unknown-runtime-instruction"));
    assert_eq!(component.stats.unsupported_runtime_calls, 2);
}

#[test]
fn rejects_an_unknown_runtime_effect_inside_a_restored_listener() {
    let source = r#"
        import * as core from "@angular/core";

        class UnsafeRestoredListenerComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: UnsafeRestoredListenerComponent,
                selectors: [["unsafe-restored-listener"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(0, "button");
                        core.ɵɵlistener("click", function() {
                            core.ɵɵrestoreView(savedView);
                            core.unknownRuntimeEffect();
                            return core.ɵɵresetView(context.select());
                        });
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("an unsafe restored listener should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MalformedInstruction
            && issue.instruction.as_deref() == Some("ɵɵlistener")
            && issue
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("unsupported Ivy runtime call"))
    }));
    assert!(!component.source.contains("unknownRuntimeEffect"));
}

#[test]
fn rejects_unreachable_effects_after_a_restored_listener_return() {
    let source = r#"
        import * as core from "@angular/core";

        class UnreachableRestoredListenerComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: UnreachableRestoredListenerComponent,
                selectors: [["unreachable-restored-listener"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        const savedView = core.ɵɵgetCurrentView();
                        core.ɵɵelementStart(0, "button");
                        core.ɵɵlistener("click", function() {
                            core.ɵɵrestoreView(savedView);
                            return core.ɵɵresetView(context.select());
                            context.afterReturn();
                        });
                        core.ɵɵelementEnd();
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("an unreachable restored-listener effect should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(component.issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MalformedInstruction
            && issue.instruction.as_deref() == Some("ɵɵlistener")
            && issue
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("final handler statement"))
    }));
    assert!(!component.source.contains("afterReturn"));
    assert!(!component.source.contains("(click)=\"select()\""));
}

#[test]
fn recovers_builtin_repeater_track_expressions() {
    let source = r#"
        import * as core from "@angular/core";

        function RowTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "span");
            }
        }

        class IndexTrackComponent {
            items = [];
            static ɵcmp = core.ɵɵdefineComponent({
                type: IndexTrackComponent,
                selectors: [["index-track"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵrepeaterCreate(
                            0, RowTemplate, 1, 0, "span", null,
                            core.ɵɵrepeaterTrackByIndex, false
                        );
                    }
                    if (rf & 2) {
                        core.ɵɵrepeater(context.items);
                    }
                },
            });
        }

        class IdentityTrackComponent {
            items = [];
            static ɵcmp = core.ɵɵdefineComponent({
                type: IdentityTrackComponent,
                selectors: [["identity-track"]],
                template: function(rf, context) {
                    if (rf & 1) {
                        core.ɵɵrepeaterCreate(
                            0, RowTemplate, 1, 0, "span", null,
                            core.ɵɵrepeaterTrackByIdentity, false
                        );
                    }
                    if (rf & 2) {
                        core.ɵɵrepeater(context.items);
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("built-in repeater track functions should be analyzed");
    let by_selector = report
        .components
        .iter()
        .map(|component| (component.selector.as_str(), component))
        .collect::<HashMap<_, _>>();

    assert_eq!(
        by_selector["index-track"].completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert!(by_selector["index-track"]
        .source
        .contains("@for (item of items; track $index) {"));
    assert_eq!(
        by_selector["identity-track"].completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert!(by_selector["identity-track"]
        .source
        .contains("@for (item of items; track item) {"));
}

#[test]
fn failed_repeater_recovery_discards_staged_child_view_diagnostics() {
    let source = r#"
        import * as core from "@angular/core";

        function RowTemplate(rf) {
            if (rf & 1) {
                core.ɵɵelement(0, "span");
            }
        }

        class InvalidTrackComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: InvalidTrackComponent,
                selectors: [["invalid-track"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵrepeaterCreate(
                            0, RowTemplate, 1, 0, "span", null, {}
                        );
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("the invalid repeater component should remain analyzable");
    let component = &report.components[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert_eq!(component.stats.runtime_calls_observed, 1);
    assert_eq!(component.stats.rendered_instruction_calls, 0);
    assert_eq!(component.stats.malformed_instruction_calls, 1);
    assert!(!component.source.contains("@for"));
    assert!(!component.source.contains("<span"));
}

#[test]
fn recovers_a_structurally_renamed_projection_selector_field() {
    let source = r#"
        import * as core from "@angular/core";

        class RenamedProjectionComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                a: RenamedProjectionComponent,
                b: [["renamed-projection"]],
                c: ["[projected-extra]"],
                d: function(rf) {
                    if (rf & 1) {
                        core.ɵɵprojectionDef([[["", "projected-extra", ""]]]);
                        core.ɵɵprojection(0);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("renamed projection descriptor fields should be analyzed");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains(r#"<ng-content select="[projected-extra]" />"#));
}

#[test]
fn recovers_artifact_imports_local_helpers_and_compiled_dependencies() {
    let source = r#"
        import {
            signal as state,
            ɵɵdefineComponent as define,
            ɵɵelement as element,
            ɵɵtext as text,
            ɵɵadvance as advance,
            ɵɵtextInterpolate as interpolate,
        } from "@angular/core";
        import { UpperCasePipe as Upper } from "@angular/common";
        import { formatTitle, normalize } from "./format.js";

        const suffix = "!";
        const unused = "not part of the artifact";
        function decorate(value) {
            return normalize(value) + suffix;
        }

        class SupportComponent {
            title = state(decorate("ready"));

            static ɵcmp = define({
                type: SupportComponent,
                selectors: [["support-card"]],
                template: function(rf, ctx) {
                    if (rf & 1) {
                        element(0, "p");
                        text(1);
                    }
                    if (rf & 2) {
                        advance(1);
                        interpolate(formatTitle(ctx.title));
                    }
                },
                dependencies: [Upper],
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("artifact support declarations should be recoverable");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains(r#"import { Component, signal } from "@angular/core";"#));
    assert!(component
        .source
        .contains(r#"import { UpperCasePipe as Upper } from "@angular/common";"#));
    assert!(component
        .source
        .contains(r#"import { normalize } from "./format.js";"#));
    assert!(component
        .source
        .contains(r#"import { formatTitle } from "./format.js";"#));
    assert!(component.source.contains(r#"const suffix = "!";"#));
    assert!(component.source.contains("function decorate(value)"));
    assert!(component
        .source
        .contains(r#"title = signal(decorate("ready"))"#));
    assert!(component.source.contains("imports: [Upper]"));
    assert!(component.source.contains("{{ formatTitle(title) }}"));
    assert!(!component.source.contains("not part of the artifact"));
    assert!(!component.source.contains("ɵɵdefineComponent"));
    assert!(!component.source.contains("ɵɵelement"));
    assert!(!component.source.contains("ɵɵtextInterpolate"));
}

#[test]
fn groups_sibling_components_and_relationships_into_one_module_artifact() {
    let source = r#"
        import * as core from "@angular/core";

        const sharedLabel = value => value.toUpperCase();

        class a {
            label = sharedLabel("child");

            static compiled = core.ɵɵdefineComponent({
                type: a,
                selectors: [["child-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "span");
                    }
                },
            });
        }

        class b {
            childType = a;
            label = sharedLabel("parent");

            static compiled = core.ɵɵdefineComponent({
                type: b,
                selectors: [["parent-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "main");
                    }
                },
                dependencies: [a],
            });
        }
    "#;

    let report = analyze_angular_components_from_modules(
        &[AngularModuleSource {
            filename: "feature.js",
            source,
        }],
        AngularRecoveryOptions::default(),
    )
    .expect("sibling components should recover as one module");

    assert_eq!(report.components.len(), 2);
    assert_eq!(report.modules.len(), 1);
    let module = &report.modules[0];
    assert_eq!(module.module_index, 0);
    assert_eq!(module.component_indices, vec![0, 1]);
    assert_eq!(
        module.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        module.issues,
        module.source,
    );
    assert_eq!(
        module
            .source
            .matches(r#"import { Component } from "@angular/core";"#)
            .count(),
        1
    );
    assert_eq!(module.source.matches("const sharedLabel =").count(), 1);
    assert_eq!(module.source.matches("@Component({").count(), 2);
    assert!(module.source.contains("export class ChildCardComponent"));
    assert!(module.source.contains("export class ParentCardComponent"));
    assert!(module.source.contains("childType = ChildCardComponent;"));
    assert!(module.source.contains("imports: [ChildCardComponent]"));
    assert!(!module
        .source
        .contains("Unresolved artifact-local symbols: a"));
    assert_typescript_parses(&module.source);
}

#[test]
fn records_cross_module_component_relationships_from_esm_edges() {
    let child = r#"
        import * as core from "@angular/core";

        export class a {
            static compiled = core.ɵɵdefineComponent({
                type: a,
                selectors: [["child-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "span");
                    }
                },
            });
        }
    "#;
    let parent = r#"
        import * as core from "@angular/core";
        import { a as c } from "./child.js";

        class d {
            static compiled = core.ɵɵdefineComponent({
                type: d,
                selectors: [["child-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "aside");
                    }
                },
            });
        }

        export class b {
            childType = c;

            static compiled = core.ɵɵdefineComponent({
                type: b,
                selectors: [["parent-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "main");
                    }
                },
                dependencies: [c],
            });
        }
    "#;

    let report = analyze_angular_components_from_modules(
        &[
            AngularModuleSource {
                filename: "src/child.js",
                source: child,
            },
            AngularModuleSource {
                filename: "src/parent.js",
                source: parent,
            },
        ],
        AngularRecoveryOptions::default(),
    )
    .expect("proven ESM component edges should be linked");

    assert_eq!(report.modules.len(), 2);
    let parent = &report.modules[1];
    assert_eq!(
        parent.dependencies,
        [RecoveredAngularModuleDependency {
            component_index: 2,
            target_component_index: 0,
            target_module_index: 0,
            target_name: "ChildCardComponent".to_string(),
            local_name: "ChildCardComponent_2".to_string(),
        }]
    );
    assert!(parent.source.contains("export class ChildCardComponent {"));
    assert!(parent.source.contains("imports: [ChildCardComponent_2]"));
    assert!(parent.source.contains("childType = ChildCardComponent_2;"));
    assert!(!parent.source.contains(r#"from "./child.js""#));
    assert_typescript_parses(&parent.source);
}

#[test]
fn disambiguates_inferred_sibling_component_names() {
    let source = r#"
        import * as core from "@angular/core";

        class a {
            static compiled = core.ɵɵdefineComponent({
                type: a,
                selectors: [["same-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "span");
                    }
                },
            });
        }

        class b {
            childType = a;

            static compiled = core.ɵɵdefineComponent({
                type: b,
                selectors: [["same-card"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵelement(0, "main");
                    }
                },
                dependencies: [a],
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("duplicate inferred names should remain recoverable");
    assert_eq!(
        report
            .components
            .iter()
            .map(|component| component.name.as_str())
            .collect::<Vec<_>>(),
        ["SameCardComponent", "SameCardComponent_2"]
    );
    let module = &report.modules[0];
    assert!(module.source.contains("export class SameCardComponent {"));
    assert!(module.source.contains("export class SameCardComponent_2 {"));
    assert!(module.source.contains("childType = SameCardComponent;"));
    assert!(module.source.contains("imports: [SameCardComponent]"));
    assert_typescript_parses(&module.source);
}

#[test]
fn refuses_a_local_helper_with_an_impure_dependency_closure() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        const runtimeConfig = makeConfig();
        function decorate(value) {
            return runtimeConfig.format(value);
        }

        class ConservativeSupportComponent {
            label = decorate("ready");

            static ɵcmp = define({
                type: ConservativeSupportComponent,
                selectors: [["conservative-support"]],
                template: function(rf) {
                    if (rf & 1) {
                        element(0, "p");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("unsupported helper closures should not prevent component recovery");
    let component = &recovered[0];

    assert!(component
        .source
        .contains("// Unresolved artifact-local symbols: decorate"));
    assert!(!component.source.contains("function decorate(value)"));
    assert!(!component.source.contains("makeConfig()"));
}

#[test]
fn escapes_template_and_style_literals_without_creating_interpolation() {
    let source = r#"
        import * as core from "@angular/core";

        class LiteralSafetyComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: LiteralSafetyComponent,
                selectors: [["literal-safety"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵtext(0, "\\${globalThis.templateInjected = true}`");
                    }
                },
                styles: [
                    "\\${globalThis.styleInjected = true}` { display: block; }"
                ],
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("literal escaping fixture should recover");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete
    );
    assert!(component
        .source
        .contains(r"\\\${globalThis.templateInjected = true}\`"));
    assert!(component
        .source
        .contains(r"\\\${globalThis.styleInjected = true}\`"));
    assert_typescript_parses(&component.source);
}

#[test]
fn reconstructs_structurally_named_angular_selector_matrices() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class SelectorMatrixComponent {
            static compiled = define({
                type: SelectorMatrixComponent,
                H: [
                    ["button", "fixtureAction", ""],
                    ["a", "fixtureAction", ""],
                ],
                A: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "span");
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("selector matrix fixture should parse");

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].selector,
        "button[fixtureAction],a[fixtureAction]"
    );
    assert!(recovered[0]
        .source
        .contains("selector: \"button[fixtureAction],a[fixtureAction]\""));
}

#[test]
fn reconstructs_selector_flags_and_rejects_incomplete_rows() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelement as element,
        } from "@angular/core";

        class FlagSelectorComponent {
            static compiled = define({
                type: FlagSelectorComponent,
                selectors: [[
                    "button",
                    "role", "action",
                    8, "primary",
                    3, "disabled", "",
                ]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "span");
                    }
                },
            });
        }

        class IncompleteSelectorComponent {
            static compiled = define({
                type: IncompleteSelectorComponent,
                H: [["title", "value"]],
                A: function(renderFlags) {
                    if (renderFlags & 1) {
                        element(0, "span");
                    }
                },
            });
        }
    "#;

    let report = analyze_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("selector flag fixture should parse");

    assert_eq!(report.components.len(), 1);
    assert_eq!(
        report.components[0].selector,
        "button[role=\"action\"].primary:not([disabled])"
    );
    assert_eq!(report.stats.rejected_component_candidates, 1);
}

#[test]
fn recovers_class_and_style_map_bindings() {
    let source = r#"
        import {
            ɵɵclassMap as classMap,
            ɵɵdefineComponent as define,
            ɵɵelement as element,
            ɵɵstyleMap as styleMap,
        } from "@angular/core";

        class StylingMapComponent {
            classes = "primary raised";
            styles = "color: rebeccapurple";

            static compiled = define({
                type: StylingMapComponent,
                selectors: [["styling-map"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        element(0, "button");
                    }
                    if (renderFlags & 2) {
                        classMap(context.classes);
                        styleMap(context.styles);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("styling map instructions should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"[class]="classes""#));
    assert!(component.source.contains(r#"[style]="styles""#));
}

#[test]
fn infers_closure_renamed_class_map_with_unobserved_style_pair() {
    let source = r#"
        runtime.component = function(definition) {
            return Object.assign({}, definition);
        };
        runtime.element = function(index, name, attrs) {
            createElement(index, name, attrs);
            return runtime.element;
        };
        runtime.checkStylingMap = function(setKey, parse, value, isClass) {};
        runtime.styleMap = function(value) {
            runtime.checkStylingMap(
                runtime.setStyle,
                runtime.parseStyle,
                value,
                false
            );
        };
        runtime.classMap = function(value) {
            runtime.checkStylingMap(
                runtime.setClass,
                runtime.parseClass,
                value,
                true
            );
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelement": runtime.element,
        };

        class RenamedStylingMapComponent {
            classes = "primary raised";
            styles = "color: rebeccapurple";

            static compiled = runtime.component({
                type: RenamedStylingMapComponent,
                selectors: [["renamed-styling-map"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.element(0, "button");
                    }
                    if (renderFlags & 2) {
                        runtime.classMap(context.classes);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-renamed styling map family should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"[class]="classes""#));
}

#[test]
fn does_not_infer_a_styling_map_from_a_lone_boolean_wrapper() {
    let source = r#"
        runtime.component = function(definition) {
            return Object.assign({}, definition);
        };
        runtime.element = function(index, name, attrs) {
            createElement(index, name, attrs);
            return runtime.element;
        };
        runtime.lookalike = function(value) {
            runtime.unknownHelper(
                runtime.firstCallback,
                runtime.secondCallback,
                value,
                true
            );
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelement": runtime.element,
        };

        class LoneWrapperComponent {
            value = "primary";

            static compiled = runtime.component({
                type: LoneWrapperComponent,
                selectors: [["lone-wrapper"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.element(0, "button");
                    }
                    if (renderFlags & 2) {
                        runtime.lookalike(context.value);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("lone wrapper fixture should parse");
    let component = &recovered[0];

    assert_eq!(component.completeness, AngularRecoveryCompleteness::Partial);
    assert!(!component.source.contains(r#"[class]="value""#));
    assert!(component
        .issues
        .iter()
        .any(|issue| issue.kind == AngularRecoveryIssueKind::UnknownRuntimeInstruction));
}

#[test]
fn infers_container_i18n_and_binding_families_from_renamed_runtime_shapes() {
    let source = r#"
        runtime.component = function(definition) {
            return Object.assign({}, definition);
        };
        runtime.elementStart = function(index, name, attrs, refs) {
            createElement(index, name, attrs, refs);
            return runtime.elementStart;
        };
        runtime.elementEnd = function() {
            closeElement();
            return runtime.elementEnd;
        };
        runtime.text = function(index, value = "") {
            createText(index, value);
        };
        runtime.advance = function(delta = 1) {
            selectIndex(delta);
        };
        runtime.interpolate = function(value) {
            updateText(value);
        };

        runtime.containerStart = function(index, attrs, refs) {
            createContainer(index, "ng-container", attrs, refs);
            return runtime.containerStart;
        };
        runtime.containerEnd = function() {
            closeContainer();
            return runtime.containerEnd;
        };
        runtime.container = function(index, attrs, refs) {
            runtime.containerStart(index, attrs, refs);
            runtime.containerEnd();
            return runtime.container;
        };

        runtime.i18nStart = function(index, message, subTemplate = -1) {
            startMessage(index, message, subTemplate);
        };
        runtime.i18nEnd = function() {
            finishMessage();
        };
        runtime.i18n = function(index, message, subTemplate) {
            runtime.i18nStart(index, message, subTemplate);
            runtime.i18nEnd();
        };
        runtime.i18nExp = function(value) {
            bindMessage(value);
            return runtime.i18nExp;
        };
        runtime.i18nApply = function(index) {
            try {
                applyMessage(index);
            } finally {
                finishBindings();
            }
        };

        runtime.styleCore = function(name, value, suffix, isClass) {};
        runtime.style = function(name, value, suffix) {
            runtime.styleCore(name, value, suffix, false);
            return runtime.style;
        };
        runtime.className = function(name, value) {
            runtime.styleCore(name, value, null, true);
            return runtime.className;
        };
        runtime.attribute = function(name, value, sanitizer, namespace) {
            const view = currentView();
            const binding = nextBinding();
            const node = selectedNode();
            writeAttribute(view, node, namespace, name, value, sanitizer);
            return runtime.attribute;
        };
        runtime.property = function(name, value, sanitizer) {
            const view = currentView();
            if (bindingChanged(view, value)) {
                writeProperty(view.node, view, name, value, view[0], sanitizer);
            }
            return runtime.property;
        };

        runtime.public = {
            "ɵɵdefineComponent": runtime.component,
            "ɵɵelementStart": runtime.elementStart,
            "ɵɵelementEnd": runtime.elementEnd,
            "ɵɵtext": runtime.text,
            "ɵɵadvance": runtime.advance,
            "ɵɵtextInterpolate": runtime.interpolate,
        };

        class StructuralFamiliesComponent {
            label = "Reader";
            active = true;
            width = 120;
            disabled = false;

            static compiled = runtime.component({
                type: StructuralFamiliesComponent,
                selectors: [["structural-families"]],
                B: () => {
                    const plain = $localize`Hello, localized world!`;
                    const bound = $localize`Hello, ${"\uFFFD0\uFFFD"}:INTERPOLATION:!`;
                    return [plain, bound];
                },
                A: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.elementStart(0, "button");
                        runtime.text(1, "Bound");
                        runtime.elementEnd();
                        runtime.containerStart(2);
                        runtime.elementStart(3, "span");
                        runtime.text(4, "Grouped");
                        runtime.elementEnd();
                        runtime.containerEnd();
                        runtime.elementStart(5, "p");
                        runtime.i18n(6, 0);
                        runtime.elementEnd();
                        runtime.elementStart(7, "p");
                        runtime.i18n(8, 1);
                        runtime.elementEnd();
                    }
                    if (renderFlags & 2) {
                        runtime.style("width", context.width, "px");
                        runtime.className("active", context.active);
                        runtime.attribute("aria-label", context.label);
                        runtime.property("disabled", context.disabled);
                        runtime.advance(8);
                        runtime.i18nExp(context.label);
                        runtime.i18nApply(8);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("renamed Angular runtime families should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("[style.width.px]=\"width\""));
    assert!(component.source.contains("[class.active]=\"active\""));
    assert!(component.source.contains("[attr.aria-label]=\"label\""));
    assert!(component.source.contains("[disabled]=\"disabled\""));
    assert!(component.source.contains("<ng-container>"));
    assert!(component
        .source
        .contains("<p i18n>Hello, localized world!</p>"));
    assert!(component.source.contains("<p i18n>Hello, {{ label }}!</p>"));
}

#[test]
fn selects_a_unique_direct_expression_i18n_constant_factory() {
    let source = r#"
        import {
            ɵɵdefineComponent as define,
            ɵɵelementStart as elementStart,
            ɵɵelementEnd as elementEnd,
            ɵɵi18n as i18n,
        } from "@angular/core";

        function localize(message) {
            return message;
        }

        class DirectI18nFactoryComponent {
            static compiled = define({
                type: DirectI18nFactoryComponent,
                selectors: [["direct-i18n-factory"]],
                constantsFactory: () => [localize("A readable message")],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        elementStart(0, "p");
                        i18n(1, 0);
                        elementEnd();
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("direct i18n constant factory fixture should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("<p i18n>A readable message</p>"));
}

#[test]
fn resolves_closure_assigned_component_and_parent_context_aliases() {
    let source = r#"
        import * as core from "@angular/core";

        class ContextAliasComponent {
            value = "current";
            title = "parent";

            static compiled = core.ɵɵdefineComponent({
                type: ContextAliasComponent,
                selectors: [["context-alias"]],
                template: function(renderFlags, context) {
                    var cachedValue;
                    if (renderFlags & 1) {
                        core.ɵɵelement(0, "button");
                    }
                    if (renderFlags & 2) {
                        cachedValue = context.value;
                        renderFlags = core.ɵɵnextContext().title;
                        core.ɵɵproperty(
                            "aria-label",
                            cachedValue + " / " + renderFlags
                        );
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-style context aliases should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(
        component
            .source
            .contains(r#"<button [aria-label]="value + &quot; / &quot; + title"></button>"#),
        "{}",
        component.source
    );
}

#[test]
fn infers_closure_renamed_let_runtime_family() {
    let source = r#"
        import * as core from "@angular/core";

        const sentinel = {};
        const state = { view: [] };

        _.d = function(slot) {
            marker("NgLet");
            const view = currentView();
            slot += 27;
            const node = allocate(view, slot, 128, null, null);
            attach(node, false);
            write(view, selectedIndex(), slot, sentinel);
            return _.d;
        };

        _.s = function(value) {
            write(currentView(), selectedIndex(), value);
            return value;
        };

        _.r = function(slot) {
            slot = state.view[27 + slot];
            if (slot === sentinel) {
                throw new Error(314);
            }
            return slot;
        };

        function LetView(renderFlags) {
            if (renderFlags & 1) {
                core.ɵɵelementStart(0, "span");
                core.ɵɵtext(1);
                core.ɵɵelementEnd();
            }
            if (renderFlags & 2) {
                const a = _.r(0);
                core.ɵɵadvance();
                core.ɵɵtextInterpolate(a);
            }
        }

        class ClosureLetComponent {
            prefix = "Status: ";
            label = "ready";
            active = true;

            static compiled = core.ɵɵdefineComponent({
                type: ClosureLetComponent,
                selectors: [["closure-let"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        _.d(0);
                        core.ɵɵelementStart(1, "p");
                        core.ɵɵtext(2);
                        core.ɵɵelementEnd();
                        core.ɵɵtemplate(3, LetView, 2, 1, "span");
                    }
                    if (renderFlags & 2) {
                        _.s(context.prefix + context.label);
                        const a = _.r(0);
                        core.ɵɵadvance(2);
                        core.ɵɵtextInterpolate(a);
                        core.ɵɵadvance();
                        core.ɵɵconditional(context.active ? 3 : -1);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-renamed @let helpers should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("@let value = prefix + label;"));
    assert!(
        component.source.contains("{{ value }}"),
        "{}",
        component.source
    );
    assert!(component.source.contains("@if (active) {"));
}

#[test]
fn shares_proven_implicit_context_properties_across_embedded_views() {
    let source = r#"
        import * as core from "@angular/core";

        function ObjectItemView(renderFlags, context) {
            if (renderFlags & 1) {
                core.ɵɵelement(0, "article");
            }
            if (renderFlags & 2) {
                const item = context.V;
                core.ɵɵproperty("title", item.name);
            }
        }

        function PrimitiveItemView(renderFlags, context) {
            if (renderFlags & 1) {
                core.ɵɵtext(0);
            }
            if (renderFlags & 2) {
                const value = context.V;
                core.ɵɵtextInterpolate(value);
            }
        }

        class SharedImplicitContextComponent {
            static compiled = core.ɵɵdefineComponent({
                type: SharedImplicitContextComponent,
                selectors: [["shared-implicit-context"]],
                template: function(renderFlags) {
                    if (renderFlags & 1) {
                        core.ɵɵtemplate(0, ObjectItemView, 1, 1);
                        core.ɵɵtemplate(1, PrimitiveItemView, 1, 1);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("shared implicit-context fixture should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"[title]="item.name""#));
    assert!(component.source.contains("{{ value }}"));
}

#[test]
fn recovers_a_closure_inlined_local_reference_slot() {
    let source = r#"
        import * as core from "@angular/core";

        class InlinedReferenceComponent {
            static compiled = core.ɵɵdefineComponent({
                type: InlinedReferenceComponent,
                selectors: [["inlined-reference"]],
                consts: [["action", ""]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        core.ɵɵelementStart(0, "button", null, 0);
                        core.ɵɵtext(2, "Action");
                        core.ɵɵelementEnd();
                        core.ɵɵelementStart(3, "span");
                        core.ɵɵtext(4);
                        core.ɵɵelementEnd();
                    }
                    if (renderFlags & 2) {
                        renderFlags = runtimeState.currentView[28];
                        core.ɵɵadvance(4);
                        core.ɵɵtextInterpolate(renderFlags.disabled);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("inlined local reference fixture should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("<button #action>Action</button>"));
    assert!(component
        .source
        .contains("<span>{{ action.disabled }}</span>"));
}

#[test]
fn infers_an_optimized_three_call_pipe_binding() {
    let source = r#"
        runtime.define = function(definition) { return definition; };
        runtime.start = function(index, name) {
            createElement(index, name);
            return runtime.start;
        };
        runtime.end = function() {
            closeElement();
            return runtime.end;
        };
        runtime.text = function(index, value = "") {
            createText(index, value);
        };
        runtime.pipe = function(index, name) {
            createPipe(index, name);
        };
        runtime.advance = function(delta = 1) {
            selectIndex(delta);
        };
        runtime.interpolate = function(value) {
            updateText(value);
        };
        runtime.bind = function(slot, binding, value) {
            const view = currentView();
            const pipe = loadPipe(view, slot);
            return invokePipe(pipe, binding, value);
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
            "ɵɵpipe": runtime.pipe,
            "ɵɵadvance": runtime.advance,
            "ɵɵtextInterpolate": runtime.interpolate,
        };

        class OptimizedPipeComponent {
            label = "reader";

            static compiled = runtime.define({
                type: OptimizedPipeComponent,
                selectors: [["optimized-pipe"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.start(0, "p");
                        runtime.text(1);
                        runtime.pipe(2, "uppercase");
                        runtime.end();
                    }
                    if (renderFlags & 2) {
                        runtime.advance();
                        runtime.interpolate(
                            runtime.bind(2, 1, context.label)
                        );
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("optimized pipe binding fixture should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("<p>{{ label | uppercase }}</p>"));
}

#[test]
fn infers_and_expands_closure_renamed_pure_function_bindings_in_container_only_views() {
    let source = r#"
        runtime.define = function(definition) {
            return definition;
        };
        runtime.container = function(index) {
            createContainer(index);
        };
        runtime.property = function(name, value, sanitizer) {
            writeProperty(name, value, sanitizer);
            return runtime.property;
        };
        runtime.pure0 = function(slot, callback) {
            return cacheValue(
                getCurrentView(),
                getBindingRoot(),
                slot,
                callback()
            );
        };
        runtime.pure1 = function(slot, callback, value) {
            return cacheValue(
                getCurrentView(),
                getBindingRoot(),
                slot,
                callback(value)
            );
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelementContainer": runtime.container,
            "ɵɵproperty": runtime.property,
        };

        const staticOptions = () => ({ fixed: true });
        const dynamicOptions = (input) => ({ value: input });

        class PureBindingComponent {
            value = "reader";

            static compiled = runtime.define({
                type: PureBindingComponent,
                selectors: [["pure-binding"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.container(0);
                    }
                    if (renderFlags & 2) {
                        runtime.property(
                            "staticOptions",
                            runtime.pure0(0, staticOptions)
                        );
                        runtime.property(
                            "dynamicOptions",
                            runtime.pure1(1, dynamicOptions, context.value)
                        );
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-renamed pure-function fixtures should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains("[staticOptions]="));
    assert!(component.source.contains("fixed: true"));
    assert!(component.source.contains("[dynamicOptions]="));
    assert!(component.source.contains("value: value"));
    assert!(!component.source.contains("runtime.pure"));
}

#[test]
fn infers_closure_renamed_expression_interpolation_from_text_family_evidence() {
    let source = r#"
        runtime.define = function(definition) {
            return definition;
        };
        runtime.start = function(index, name) {
            createElement(index, name);
        };
        runtime.end = function() {
            closeElement();
        };
        runtime.text = function(index, value = "") {
            createText(index, value);
        };
        runtime.property = function(name, value) {
            writeProperty(name, value);
            return runtime.property;
        };
        runtime.attribute = function(name, value) {
            writeAttribute(name, value);
            return runtime.attribute;
        };
        runtime.advance = function(delta = 1) {
            selectNode(delta);
        };
        runtime.interpolateLow = function(view, prefix, value, suffix) {
            return runtime.changed(view, runtime.nextBinding(), value)
                ? prefix + runtime.stringify(value) + suffix
                : runtime.noChange;
        };
        runtime.textOne = function(prefix, value, suffix) {
            const rendered = runtime.interpolateLow(
                runtime.getView(),
                prefix,
                value,
                suffix
            );
            writeText(runtime.getView(), rendered);
            return runtime.textOne;
        };
        runtime.textValue = function(value) {
            runtime.textOne("", value);
            return runtime.textValue;
        };
        runtime.interpolateValue = function(value) {
            return runtime.changed(
                runtime.getView(),
                runtime.nextBinding(),
                value
            ) ? runtime.stringify(value) : runtime.noChange;
        };
        runtime.interpolateOne = function(prefix, value, suffix = "") {
            return runtime.interpolateLow(
                runtime.getView(),
                prefix,
                value,
                suffix
            );
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵtext": runtime.text,
            "ɵɵproperty": runtime.property,
            "ɵɵattribute": runtime.attribute,
            "ɵɵadvance": runtime.advance,
        };

        class InterpolationComponent {
            label = "reader";

            static compiled = runtime.define({
                type: InterpolationComponent,
                selectors: [["interpolation-card"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.start(0, "button");
                        runtime.text(1);
                        runtime.end();
                    }
                    if (renderFlags & 2) {
                        runtime.property(
                            "title",
                            runtime.interpolateValue(context.label)
                        );
                        runtime.attribute(
                            "aria-label",
                            runtime.interpolateOne(
                                "Hello ",
                                context.label,
                                "!"
                            )
                        );
                        runtime.advance();
                        runtime.textValue(context.label);
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure-renamed expression-interpolation fixture should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component.source.contains(r#"[title]="\`\${label}\`""#));
    assert!(component
        .source
        .contains(r#"[attr.aria-label]="\`Hello \${label}!\`""#));
    assert!(component.source.contains(">{{ label }}</button>"));
    assert!(!component.source.contains("runtime.interpolate"));
}

#[test]
fn preserves_closure_namespace_application_helpers_in_bindings_and_listeners() {
    let source = r#"
        runtime.define = function(definition) {
            return definition;
        };
        runtime.start = function(index, name) {
            createElement(index, name);
        };
        runtime.end = function() {
            closeElement();
        };
        runtime.listener = function(name, handler) {
            listen(name, handler);
            return runtime.listener;
        };
        runtime.property = function(name, value) {
            writeProperty(name, value);
            return runtime.property;
        };
        runtime.applicationHelper = function(value, event) {
            return event ? value + event.type : value.trim();
        };
        runtime.public = {
            "ɵɵdefineComponent": runtime.define,
            "ɵɵelementStart": runtime.start,
            "ɵɵelementEnd": runtime.end,
            "ɵɵlistener": runtime.listener,
            "ɵɵproperty": runtime.property,
        };

        class ApplicationHelperComponent {
            value = "reader";

            static compiled = runtime.define({
                type: ApplicationHelperComponent,
                selectors: [["application-helper-card"]],
                template: function(renderFlags, context) {
                    if (renderFlags & 1) {
                        runtime.start(0, "button");
                        runtime.listener("click", function($event) {
                            return runtime.applicationHelper(
                                context.value,
                                $event
                            );
                        });
                        runtime.end();
                    }
                    if (renderFlags & 2) {
                        runtime.property(
                            "value",
                            runtime.applicationHelper(context.value)
                        );
                    }
                },
            });
        }
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("Closure application-helper fixture should parse");
    let component = &recovered[0];

    assert_eq!(
        component.completeness,
        AngularRecoveryCompleteness::Complete,
        "issues: {:#?}\n{}",
        component.issues,
        component.source,
    );
    assert!(component
        .source
        .contains(r#"(click)="runtime.applicationHelper(value, $event)""#));
    assert!(component
        .source
        .contains(r#"[value]="runtime.applicationHelper(value)""#));
}

fn assert_typescript_parses(source: &str) {
    use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

    let cm: Lrc<SourceMap> = Default::default();
    let file = cm.new_source_file(
        FileName::Custom("recovered.angular.ts".to_string()).into(),
        source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            decorators: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*file),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser.parse_module().unwrap_or_else(|error| {
        panic!("the grouped inspection artifact should parse as TypeScript: {error:?}\n{source}")
    });
    assert!(
        parser.take_errors().is_empty(),
        "the grouped inspection artifact should parse without recovery errors"
    );
}
