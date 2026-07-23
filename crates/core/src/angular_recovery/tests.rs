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
