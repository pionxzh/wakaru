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
        class ProjectedComponent {
            static ɵcmp = core.ɵɵdefineComponent({
                type: ProjectedComponent,
                selectors: [["projected-content"]],
                template: function(rf) {
                    if (rf & 1) {
                        core.ɵɵprojection(0);
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
        .contains("<!-- Unsupported Ivy instruction: ɵɵprojection -->"));
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
                    runtime.q(1);
                }
            },
        });
    "#;

    let recovered = recover_angular_components_from_js(source, AngularRecoveryOptions::default())
        .expect("unclassified runtime calls should not prevent partial recovery");

    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].completeness,
        AngularRecoveryCompleteness::Partial
    );
    assert!(recovered[0]
        .source
        .contains("Unsupported Ivy instruction: unknown-runtime-instruction"));
    assert!(!recovered[0].source.contains("runtime.q"));
}
