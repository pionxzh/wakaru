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
    assert!(recovered[0].issues.iter().any(|issue| {
        issue.kind == AngularRecoveryIssueKind::MalformedInstruction
            && issue.instruction.as_deref() == Some("ɵɵelement")
    }));
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
        .contains(r#"import { signal as state } from "@angular/core";"#));
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
    assert!(component.source.contains("imports: [Upper]"));
    assert!(component.source.contains("{{ formatTitle(title) }}"));
    assert!(!component.source.contains("not part of the artifact"));
    assert!(!component.source.contains("ɵɵdefineComponent"));
    assert!(!component.source.contains("ɵɵelement"));
    assert!(!component.source.contains("ɵɵtextInterpolate"));
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
