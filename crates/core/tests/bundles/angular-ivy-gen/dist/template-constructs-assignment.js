import { NgFor, NgIf, UpperCasePipe } from "@angular/common";
import { model } from "@angular/core";
import * as i0 from "@angular/core";
var FixtureLetBindingsComponent_Conditional_3_Template, FixtureStructuralConstructsComponent_Conditional_1_Template, FixtureStructuralConstructsComponent_Conditional_2_Template, FixtureStructuralConstructsComponent_For_4_Template, FixtureStructuralConstructsComponent_ForEmpty_5_Template, FixtureComplexListenerComponent_For_2_Conditional_0_Template, FixtureComplexListenerComponent_For_2_Template, FixtureSubTemplateI18nComponent_ng_template_2_Template, FixtureProjectionFallbackComponent_ProjectionFallback_1_Template, FixtureProjectionFallbackComponent_ProjectionFallback_3_Template, FixtureDeferredConstructsComponent_Defer_1_Template, FixtureDeferredConstructsComponent_DeferLoading_2_Template, FixtureDeferredConstructsComponent_DeferPlaceholder_3_Template, FixtureDeferredConstructsComponent_DeferError_4_Template, FixturePrefetchIdleConstructsComponent_Defer_1_Template, FixturePrefetchIdleConstructsComponent_DeferPlaceholder_2_Template, FixtureHydrateIdleConstructsComponent_Defer_1_Template, FixtureHydrateIdleConstructsComponent_DeferPlaceholder_2_Template, FixtureLegacyStructuralConstructsComponent_p_1_Template, FixtureLegacyStructuralConstructsComponent_span_2_Template;
const _c0 = () => ({ fixed: !0 }), _c1 = (a0) => ({ label: a0 }), _c2 = (a0) => [a0];
FixtureLetBindingsComponent_Conditional_3_Template = function (rf, ctx) {
    if (rf & 1) {
        const _r1 = i0.\u0275\u0275getCurrentView();
        i0.\u0275\u0275domElementStart(0, "button", 1), i0.\u0275\u0275domListener("click", function () {
            i0.\u0275\u0275restoreView(_r1);
            const ctx_r1 = i0.\u0275\u0275nextContext(), displayLabel_r3 = i0.\u0275\u0275readContextLet(0);
            return i0.\u0275\u0275resetView(ctx_r1.activate(displayLabel_r3));
        }), i0.\u0275\u0275text(1), i0.\u0275\u0275domElementEnd();
    }
    if (rf & 2) {
        i0.\u0275\u0275nextContext();
        const displayLabel_r3 = i0.\u0275\u0275readContextLet(0);
        i0.\u0275\u0275advance(), i0.\u0275\u0275textInterpolate1(" ", displayLabel_r3, " ");
    }
};
const _c3 = [[["", "card-footer", ""]]], _c4 = ["[card-footer]"], _forTrack0 = ($index, $item) => $item.id;
FixtureStructuralConstructsComponent_Conditional_1_Template = function (rf, ctx) {
    if (rf & 1 && (i0.\u0275\u0275domElementStart(0, "h2"), i0.\u0275\u0275text(1), i0.\u0275\u0275pipe(2, "uppercase"), i0.\u0275\u0275domElementEnd()), rf & 2) {
        const ctx_r0 = i0.\u0275\u0275nextContext();
        i0.\u0275\u0275advance(), i0.\u0275\u0275textInterpolate(i0.\u0275\u0275pipeBind1(2, 1, ctx_r0.title));
    }
};
FixtureStructuralConstructsComponent_Conditional_2_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "p"), i0.\u0275\u0275text(1, "Details hidden"), i0.\u0275\u0275domElementEnd());
};
FixtureStructuralConstructsComponent_For_4_Template = function (rf, ctx) {
    if (rf & 1) {
        const _r2 = i0.\u0275\u0275getCurrentView();
        i0.\u0275\u0275domElementStart(0, "button", 2, 0), i0.\u0275\u0275domListener("click", function () {
            const item_r3 = i0.\u0275\u0275restoreView(_r2).$implicit, row_r4 = i0.\u0275\u0275reference(1), ctx_r0 = i0.\u0275\u0275nextContext();
            return i0.\u0275\u0275resetView(ctx_r0.select(row_r4, item_r3));
        }), i0.\u0275\u0275text(2), i0.\u0275\u0275domElementEnd();
    }
    if (rf & 2) {
        const item_r3 = ctx.$implicit;
        i0.\u0275\u0275advance(2), i0.\u0275\u0275textInterpolate1(" ", item_r3.label, " ");
    }
};
FixtureStructuralConstructsComponent_ForEmpty_5_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "p"), i0.\u0275\u0275text(1, "No items"), i0.\u0275\u0275domElementEnd());
};
FixtureComplexListenerComponent_For_2_Conditional_0_Template = function (rf, ctx) {
    if (rf & 1) {
        const _r1 = i0.\u0275\u0275getCurrentView();
        i0.\u0275\u0275domElementStart(0, "button", 2, 0), i0.\u0275\u0275domListener("click", function () {
            i0.\u0275\u0275restoreView(_r1);
            const button_r2 = i0.\u0275\u0275reference(1), item_r3 = i0.\u0275\u0275nextContext().$implicit, ctx_r3 = i0.\u0275\u0275nextContext(), displayLabel_r5 = i0.\u0275\u0275readContextLet(0);
            return ctx_r3.record(button_r2, item_r3, displayLabel_r5), i0.\u0275\u0275resetView(ctx_r3.active = !1);
        }), i0.\u0275\u0275text(2), i0.\u0275\u0275domElementEnd();
    }
    if (rf & 2) {
        const item_r3 = i0.\u0275\u0275nextContext().$implicit;
        i0.\u0275\u0275advance(2), i0.\u0275\u0275textInterpolate1(" ", item_r3.label, " ");
    }
};
FixtureComplexListenerComponent_For_2_Template = function (rf, ctx) {
    if (rf & 1 && i0.\u0275\u0275conditionalCreate(0, FixtureComplexListenerComponent_For_2_Conditional_0_Template, 3, 1, "button", 1), rf & 2) {
        const ctx_r3 = i0.\u0275\u0275nextContext();
        i0.\u0275\u0275conditional(ctx_r3.active ? 0 : -1);
    }
};
FixtureSubTemplateI18nComponent_ng_template_2_Template = function (rf, ctx) {
    if (rf & 1 && (i0.\u0275\u0275i18nStart(0, 0, 1), i0.\u0275\u0275element(1, "strong"), i0.\u0275\u0275i18nEnd()), rf & 2) {
        const ctx_r0 = i0.\u0275\u0275nextContext();
        i0.\u0275\u0275advance(), i0.\u0275\u0275i18nExp(ctx_r0.name), i0.\u0275\u0275i18nApply(0);
    }
};
const _c5 = [[["", "card-title", ""]], "*"], _c6 = ["[card-title]", "*"];
FixtureProjectionFallbackComponent_ProjectionFallback_1_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "h2"), i0.\u0275\u0275text(1, "Fallback title"), i0.\u0275\u0275domElementEnd());
};
FixtureProjectionFallbackComponent_ProjectionFallback_3_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "p"), i0.\u0275\u0275text(1, "Fallback body"), i0.\u0275\u0275domElementEnd());
};
FixtureDeferredConstructsComponent_Defer_1_Template = function (rf, ctx) {
    if (rf & 1 && (i0.\u0275\u0275domElementStart(0, "article"), i0.\u0275\u0275text(1), i0.\u0275\u0275domElementEnd()), rf & 2) {
        const ctx_r0 = i0.\u0275\u0275nextContext();
        i0.\u0275\u0275advance(), i0.\u0275\u0275textInterpolate(ctx_r0.title);
    }
};
FixtureDeferredConstructsComponent_DeferLoading_2_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "p"), i0.\u0275\u0275text(1, "Loading"), i0.\u0275\u0275domElementEnd());
};
FixtureDeferredConstructsComponent_DeferPlaceholder_3_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "p"), i0.\u0275\u0275text(1, "Waiting"), i0.\u0275\u0275domElementEnd());
};
FixtureDeferredConstructsComponent_DeferError_4_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "p"), i0.\u0275\u0275text(1, "Failed"), i0.\u0275\u0275domElementEnd());
};
FixturePrefetchIdleConstructsComponent_Defer_1_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "article"), i0.\u0275\u0275text(1, "Prefetched content"), i0.\u0275\u0275domElementEnd());
};
FixturePrefetchIdleConstructsComponent_DeferPlaceholder_2_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "button", 0), i0.\u0275\u0275text(1, "Load prefetched content"), i0.\u0275\u0275domElementEnd());
};
FixtureHydrateIdleConstructsComponent_Defer_1_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "article"), i0.\u0275\u0275text(1, "Hydrated content"), i0.\u0275\u0275domElementEnd());
};
FixtureHydrateIdleConstructsComponent_DeferPlaceholder_2_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275domElementStart(0, "button", 0), i0.\u0275\u0275text(1, "Load hydrated content"), i0.\u0275\u0275domElementEnd());
};
FixtureLegacyStructuralConstructsComponent_p_1_Template = function (rf, ctx) {
    rf & 1 && (i0.\u0275\u0275elementStart(0, "p"), i0.\u0275\u0275text(1, "Legacy visible"), i0.\u0275\u0275elementEnd());
};
FixtureLegacyStructuralConstructsComponent_span_2_Template = function (rf, ctx) {
    if (rf & 1 && (i0.\u0275\u0275elementStart(0, "span"), i0.\u0275\u0275text(1), i0.\u0275\u0275elementEnd()), rf & 2) {
        const item_r1 = ctx.$implicit;
        i0.\u0275\u0275advance(), i0.\u0275\u0275textInterpolate(item_r1);
    }
};
class FixtureFlatBindingsComponent {
    label = "Generated label";
    prefix = "Status:";
    active = !0;
    opacity = 0.75;
    disabled = !1;
    activate(_event) {
        this.active = !this.active;
    }
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureFlatBindingsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureFlatBindingsComponent, selectors: [["fixture-flat-bindings"]], decls: 4, vars: 8, consts: [["title", "Flat bindings", 3, "click"], [3, "disabled"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "article", 0), i0.\u0275\u0275domListener("click", function ($event) {
                return ctx.activate($event);
            }), i0.\u0275\u0275domElementStart(1, "h2"), i0.\u0275\u0275text(2), i0.\u0275\u0275domElementEnd(), i0.\u0275\u0275domElement(3, "input", 1), i0.\u0275\u0275domElementEnd()), rf & 2 && (i0.\u0275\u0275styleProp("opacity", ctx.opacity), i0.\u0275\u0275classProp("active", ctx.active), i0.\u0275\u0275attribute("aria-label", ctx.label), i0.\u0275\u0275advance(2), i0.\u0275\u0275textInterpolate2("", ctx.prefix, " ", ctx.label), i0.\u0275\u0275advance(), i0.\u0275\u0275domProperty("disabled", ctx.disabled));
        }, encapsulation: 2 });
}
class FixtureContainerI18nComponent {
    name = "reader";
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureContainerI18nComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureContainerI18nComponent, selectors: [["fixture-container-i18n"]], decls: 8, vars: 1, consts: () => {
            let i18n_0;
            typeof ngI18nClosureMode < "u" && ngI18nClosureMode ? i18n_0 =
                goog.getMsg("Hello, localized world!") : i18n_0 = $localize `Hello, localized world!`;
            let i18n_1;
            return typeof ngI18nClosureMode < "u" && ngI18nClosureMode ? i18n_1 =
                goog.getMsg("Hello, {$interpolation}!", { interpolation: "\uFFFD0\uFFFD" }, { original_code: { interpolation: "{{ name }}" } }) : i18n_1 = $localize `Hello, ${"\uFFFD0\uFFFD"}:INTERPOLATION:!`, [i18n_0, i18n_1];
        }, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "section"), i0.\u0275\u0275domElementContainerStart(1), i0.\u0275\u0275domElementStart(2, "span"), i0.\u0275\u0275text(3, "Grouped content"), i0.\u0275\u0275domElementEnd(), i0.\u0275\u0275domElementContainerEnd(), i0.\u0275\u0275domElementStart(4, "p"), i0.\u0275\u0275i18n(5, 0), i0.\u0275\u0275domElementEnd(), i0.\u0275\u0275domElementStart(6, "p"), i0.\u0275\u0275i18n(7, 1), i0.\u0275\u0275domElementEnd()()), rf & 2 && (i0.\u0275\u0275advance(7), i0.\u0275\u0275i18nExp(ctx.name), i0.\u0275\u0275i18nApply(7));
        }, encapsulation: 2 });
}
class FixtureSelectorMatrixComponent {
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureSelectorMatrixComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureSelectorMatrixComponent, selectors: [["button", "fixtureAction", ""], ["a", "fixtureAction", ""]], decls: 2, vars: 0, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "span"), i0.\u0275\u0275text(1, "Selector matrix"), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixtureElementSelectorComponent {
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureElementSelectorComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureElementSelectorComponent, selectors: [["dialog", "fixtureDialog", ""]], decls: 2, vars: 0, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "span"), i0.\u0275\u0275text(1, "Element selector"), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixturePureTargetDirective {
    config;
    items;
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixturePureTargetDirective)();
    };
    static \u0275dir = i0.\u0275\u0275defineDirective({ type: FixturePureTargetDirective, selectors: [["fixture-pure-target"]], inputs: { config: "config", items: "items" } });
}
class FixturePureBindingsComponent {
    label = "reader";
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixturePureBindingsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixturePureBindingsComponent, selectors: [["fixture-pure-bindings"]], decls: 4, vars: 12, consts: [[3, "config"], [3, "config", "items"], [3, "title"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275element(0, "fixture-pure-target", 0)(1, "fixture-pure-target", 1), i0.\u0275\u0275elementStart(2, "button", 2), i0.\u0275\u0275text(3, " Interpolate "), i0.\u0275\u0275elementEnd()), rf & 2 && (i0.\u0275\u0275property("config", i0.\u0275\u0275pureFunction0(7, _c0)), i0.\u0275\u0275advance(), i0.\u0275\u0275property("config", i0.\u0275\u0275pureFunction1(8, _c1, ctx.label))("items", i0.\u0275\u0275pureFunction1(10, _c2, ctx.label)), i0.\u0275\u0275advance(), i0.\u0275\u0275attribute("data-label", i0.\u0275\u0275interpolate1("Hello ", ctx.label, "!")), i0.\u0275\u0275property("title", i0.\u0275\u0275interpolate(ctx.label)));
        }, dependencies: [FixturePureTargetDirective], encapsulation: 2 });
}
class FixtureLetBindingsComponent {
    prefix = "Status: ";
    label = "ready";
    active = !0;
    activate(label) {
        console.log(label);
    }
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureLetBindingsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureLetBindingsComponent, selectors: [["fixture-let-bindings"]], decls: 4, vars: 3, consts: [["type", "button"], ["type", "button", 3, "click"]], template: function (rf, ctx) {
            if (rf & 1 && (i0.\u0275\u0275declareLet(0), i0.\u0275\u0275domElementStart(1, "p"), i0.\u0275\u0275text(2), i0.\u0275\u0275domElementEnd(), i0.\u0275\u0275conditionalCreate(3, FixtureLetBindingsComponent_Conditional_3_Template, 2, 1, "button", 0)), rf & 2) {
                const displayLabel_r4 = i0.\u0275\u0275storeLet(ctx.prefix + ctx.label);
                i0.\u0275\u0275advance(2), i0.\u0275\u0275textInterpolate(displayLabel_r4), i0.\u0275\u0275advance(), i0.\u0275\u0275conditional(ctx.active ? 3 : -1);
            }
        }, encapsulation: 2 });
}
class FixtureNamespacesComponent {
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureNamespacesComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureNamespacesComponent, selectors: [["fixture-namespaces"]], decls: 11, vars: 0, consts: [["viewBox", "0 0 10 10"], ["cx", "5", "cy", "5", "r", "4"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275namespaceSVG(), i0.\u0275\u0275domElementStart(0, "svg", 0), i0.\u0275\u0275domElement(1, "circle", 1), i0.\u0275\u0275domElementEnd(), i0.\u0275\u0275namespaceMathML(), i0.\u0275\u0275domElementStart(2, "math")(3, "mi"), i0.\u0275\u0275text(4, "x"), i0.\u0275\u0275domElementEnd(), i0.\u0275\u0275domElementStart(5, "mo"), i0.\u0275\u0275text(6, "="), i0.\u0275\u0275domElementEnd(), i0.\u0275\u0275domElementStart(7, "mn"), i0.\u0275\u0275text(8, "1"), i0.\u0275\u0275domElementEnd()(), i0.\u0275\u0275namespaceHTML(), i0.\u0275\u0275domElementStart(9, "p"), i0.\u0275\u0275text(10, "HTML"), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixtureStructuralConstructsComponent {
    showDetails = !0;
    title = "Generated structures";
    items = [{ id: 1, label: "First" }];
    select(_row, _item) {
    }
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureStructuralConstructsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureStructuralConstructsComponent, selectors: [["fixture-structural-constructs"]], ngContentSelectors: _c4, decls: 7, vars: 2, consts: [["row", ""], ["type", "button"], ["type", "button", 3, "click"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275projectionDef(_c3), i0.\u0275\u0275domElementStart(0, "section"), i0.\u0275\u0275conditionalCreate(1, FixtureStructuralConstructsComponent_Conditional_1_Template, 3, 3, "h2")(2, FixtureStructuralConstructsComponent_Conditional_2_Template, 2, 0, "p"), i0.\u0275\u0275repeaterCreate(3, FixtureStructuralConstructsComponent_For_4_Template, 3, 1, "button", 1, _forTrack0, !1, FixtureStructuralConstructsComponent_ForEmpty_5_Template, 2, 0, "p"), i0.\u0275\u0275projection(6), i0.\u0275\u0275domElementEnd()), rf & 2 && (i0.\u0275\u0275advance(), i0.\u0275\u0275conditional(ctx.showDetails ? 1 : 2), i0.\u0275\u0275advance(2), i0.\u0275\u0275repeater(ctx.items));
        }, dependencies: [UpperCasePipe], encapsulation: 2 });
}
class FixtureComplexListenerComponent {
    prefix = "Selected: ";
    suffix = "item";
    active = !0;
    items = [{ id: 1, label: "First" }];
    record(_button, _item, _displayLabel) {
        console.log(_button, _item, _displayLabel);
    }
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureComplexListenerComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureComplexListenerComponent, selectors: [["fixture-complex-listener"]], decls: 3, vars: 1, consts: [["button", ""], ["type", "button"], ["type", "button", 3, "click"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275declareLet(0), i0.\u0275\u0275repeaterCreate(1, FixtureComplexListenerComponent_For_2_Template, 1, 1, null, null, _forTrack0)), rf & 2 && (i0.\u0275\u0275storeLet(ctx.prefix + ctx.suffix), i0.\u0275\u0275advance(), i0.\u0275\u0275repeater(ctx.items));
        }, encapsulation: 2 });
}
class FixtureStructuralI18nComponent {
    name = "reader";
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureStructuralI18nComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureStructuralI18nComponent, selectors: [["fixture-structural-i18n"]], decls: 3, vars: 1, consts: () => {
            let i18n_2;
            return typeof ngI18nClosureMode < "u" && ngI18nClosureMode ? i18n_2 =
                goog.getMsg("Hello {$startTagStrong}{$interpolation}{$closeTagStrong}!", { closeTagStrong: "\uFFFD/#2\uFFFD", interpolation: "\uFFFD0\uFFFD", startTagStrong: "\uFFFD#2\uFFFD" }, { original_code: { closeTagStrong: "</strong>", interpolation: "{{ name }}", startTagStrong: "<strong>" } }) : i18n_2 = $localize `Hello ${"\uFFFD#2\uFFFD"}:START_TAG_STRONG:${"\uFFFD0\uFFFD"}:INTERPOLATION:${"\uFFFD/#2\uFFFD"}:CLOSE_TAG_STRONG:!`, [i18n_2];
        }, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "p"), i0.\u0275\u0275i18nStart(1, 0), i0.\u0275\u0275domElement(2, "strong"), i0.\u0275\u0275i18nEnd(), i0.\u0275\u0275domElementEnd()), rf & 2 && (i0.\u0275\u0275advance(2), i0.\u0275\u0275i18nExp(ctx.name), i0.\u0275\u0275i18nApply(1));
        }, encapsulation: 2 });
}
class FixturePostprocessedStructuralI18nComponent {
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixturePostprocessedStructuralI18nComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixturePostprocessedStructuralI18nComponent, selectors: [["fixture-postprocessed-structural-i18n"]], decls: 4, vars: 0, consts: () => {
            let i18n_3;
            return typeof ngI18nClosureMode < "u" && ngI18nClosureMode ? i18n_3 =
                goog.getMsg(" Read {$startLink}First{$closeLink} and {$startLink_1}Second{$closeLink}. ", { closeLink: "[\uFFFD/#2\uFFFD|\uFFFD/#3\uFFFD]", startLink: "\uFFFD#2\uFFFD", startLink_1: "\uFFFD#3\uFFFD" }, { original_code: { closeLink: "</a>", startLink: '<a href="/first">', startLink_1: '<a href="/second">' } }) : i18n_3 = $localize ` Read ${"\uFFFD#2\uFFFD"}:START_LINK:First${"[\uFFFD/#2\uFFFD|\uFFFD/#3\uFFFD]"}:CLOSE_LINK: and ${"\uFFFD#3\uFFFD"}:START_LINK_1:Second${"[\uFFFD/#2\uFFFD|\uFFFD/#3\uFFFD]"}:CLOSE_LINK:. `, i18n_3 = i0.\u0275\u0275i18nPostprocess(i18n_3), [i18n_3, ["href", "/first"], ["href", "/second"]];
        }, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "p"), i0.\u0275\u0275i18nStart(1, 0), i0.\u0275\u0275domElement(2, "a", 1)(3, "a", 2), i0.\u0275\u0275i18nEnd(), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixtureSubTemplateI18nComponent {
    visible = !0;
    name = "reader";
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureSubTemplateI18nComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureSubTemplateI18nComponent, selectors: [["fixture-sub-template-i18n"]], decls: 3, vars: 1, consts: () => {
            let i18n_4;
            return typeof ngI18nClosureMode < "u" && ngI18nClosureMode ? i18n_4 =
                goog.getMsg(" Visible before. {$startTagNgTemplate}{$startTagStrong}Hello, {$interpolation}!{$closeTagStrong}{$closeTagNgTemplate} Visible after. ", { closeTagNgTemplate: "\uFFFD/*2:1\uFFFD", closeTagStrong: "\uFFFD/#1:1\uFFFD", interpolation: "\uFFFD0:1\uFFFD", startTagNgTemplate: "\uFFFD*2:1\uFFFD", startTagStrong: "\uFFFD#1:1\uFFFD" }, { original_code: { closeTagNgTemplate: "</ng-template>", closeTagStrong: "</strong>", interpolation: "{{ name }}", startTagNgTemplate: '<ng-template [ngIf]="visible">', startTagStrong: "<strong>" } }) : i18n_4 = $localize ` Visible before. ${"\uFFFD*2:1\uFFFD"}:START_TAG_NG_TEMPLATE:${"\uFFFD#1:1\uFFFD"}:START_TAG_STRONG:Hello, ${"\uFFFD0:1\uFFFD"}:INTERPOLATION:!${"\uFFFD/#1:1\uFFFD"}:CLOSE_TAG_STRONG:${"\uFFFD/*2:1\uFFFD"}:CLOSE_TAG_NG_TEMPLATE: Visible after. `, [i18n_4, [3, "ngIf"]];
        }, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275elementStart(0, "section"), i0.\u0275\u0275i18nStart(1, 0), i0.\u0275\u0275template(2, FixtureSubTemplateI18nComponent_ng_template_2_Template, 2, 1, "ng-template", 1), i0.\u0275\u0275i18nEnd(), i0.\u0275\u0275elementEnd()), rf & 2 && (i0.\u0275\u0275advance(2), i0.\u0275\u0275property("ngIf", ctx.visible));
        }, dependencies: [NgIf], encapsulation: 2 });
}
class FixtureProjectionFallbackComponent {
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureProjectionFallbackComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureProjectionFallbackComponent, selectors: [["fixture-projection-fallback"]], ngContentSelectors: _c6, decls: 5, vars: 0, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275projectionDef(_c5), i0.\u0275\u0275domElementStart(0, "section"), i0.\u0275\u0275projection(1, 0, null, FixtureProjectionFallbackComponent_ProjectionFallback_1_Template, 2, 0), i0.\u0275\u0275projection(3, 1, null, FixtureProjectionFallbackComponent_ProjectionFallback_3_Template, 2, 0), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixtureModelTargetDirective {
    value = model("");
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureModelTargetDirective)();
    };
    static \u0275dir = i0.\u0275\u0275defineDirective({ type: FixtureModelTargetDirective, selectors: [["fixture-model-target"]], inputs: { value: [1, "value"] }, outputs: { value: "valueChange" } });
}
class FixtureTwoWayBindingComponent {
    name = "reader";
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureTwoWayBindingComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureTwoWayBindingComponent, selectors: [["fixture-two-way-binding"]], decls: 1, vars: 1, consts: [[3, "valueChange", "value"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275elementStart(0, "fixture-model-target", 0), i0.\u0275\u0275twoWayListener("valueChange", function ($event) {
                return i0.\u0275\u0275twoWayBindingSet(ctx.name, $event) || (ctx.name = $event), $event;
            }), i0.\u0275\u0275elementEnd()), rf & 2 && i0.\u0275\u0275twoWayProperty("value", ctx.name);
        }, dependencies: [FixtureModelTargetDirective], encapsulation: 2 });
}
class FixtureAnimationBindingsComponent {
    leaveClass = "fade-out";
    started(_event) {
        console.log(_event);
    }
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureAnimationBindingsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureAnimationBindingsComponent, selectors: [["fixture-animation-bindings"]], decls: 2, vars: 0, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "div"), i0.\u0275\u0275animateLeave(function () {
                return ctx.leaveClass;
            }), i0.\u0275\u0275animateEnter("fade-in"), i0.\u0275\u0275animateEnterListener(function ($event) {
                return ctx.started($event);
            }), i0.\u0275\u0275text(1, " Animated "), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixtureDeferredConstructsComponent {
    title = "Deferred content";
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureDeferredConstructsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureDeferredConstructsComponent, selectors: [["fixture-deferred-constructs"]], decls: 7, vars: 0, template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "section"), i0.\u0275\u0275domTemplate(1, FixtureDeferredConstructsComponent_Defer_1_Template, 2, 1)(2, FixtureDeferredConstructsComponent_DeferLoading_2_Template, 2, 0)(3, FixtureDeferredConstructsComponent_DeferPlaceholder_3_Template, 2, 0)(4, FixtureDeferredConstructsComponent_DeferError_4_Template, 2, 0), i0.\u0275\u0275defer(5, 1, null, 2, 3, 4), i0.\u0275\u0275deferOnIdle(), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixturePrefetchIdleConstructsComponent {
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixturePrefetchIdleConstructsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixturePrefetchIdleConstructsComponent, selectors: [["fixture-prefetch-idle-constructs"]], decls: 5, vars: 0, consts: [["type", "button"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "section"), i0.\u0275\u0275domTemplate(1, FixturePrefetchIdleConstructsComponent_Defer_1_Template, 2, 0)(2, FixturePrefetchIdleConstructsComponent_DeferPlaceholder_2_Template, 2, 0), i0.\u0275\u0275defer(3, 1, null, null, 2), i0.\u0275\u0275deferOnInteraction(0, -1), i0.\u0275\u0275deferPrefetchOnIdle(), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixtureHydrateIdleConstructsComponent {
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureHydrateIdleConstructsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureHydrateIdleConstructsComponent, selectors: [["fixture-hydrate-idle-constructs"]], decls: 5, vars: 0, consts: [["type", "button"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275domElementStart(0, "section"), i0.\u0275\u0275domTemplate(1, FixtureHydrateIdleConstructsComponent_Defer_1_Template, 2, 0)(2, FixtureHydrateIdleConstructsComponent_DeferPlaceholder_2_Template, 2, 0), i0.\u0275\u0275enableIncrementalHydrationRuntime(), i0.\u0275\u0275defer(3, 1, null, null, 2, null, null, null, null, 1), i0.\u0275\u0275deferHydrateOnIdle(), i0.\u0275\u0275deferOnInteraction(0, -1), i0.\u0275\u0275domElementEnd());
        }, encapsulation: 2 });
}
class FixtureLegacyStructuralConstructsComponent {
    visible = !0;
    items = ["First", "Second"];
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureLegacyStructuralConstructsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureLegacyStructuralConstructsComponent, selectors: [["fixture-legacy-structural-constructs"]], decls: 3, vars: 2, consts: [[4, "ngIf"], [4, "ngFor", "ngForOf"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275elementStart(0, "section"), i0.\u0275\u0275template(1, FixtureLegacyStructuralConstructsComponent_p_1_Template, 2, 0, "p", 0)(2, FixtureLegacyStructuralConstructsComponent_span_2_Template, 2, 1, "span", 1), i0.\u0275\u0275elementEnd()), rf & 2 && (i0.\u0275\u0275advance(), i0.\u0275\u0275property("ngIf", ctx.visible), i0.\u0275\u0275advance(), i0.\u0275\u0275property("ngForOf", ctx.items));
        }, dependencies: [NgIf, NgFor], encapsulation: 2 });
}
export { FixtureAnimationBindingsComponent, FixtureComplexListenerComponent, FixtureContainerI18nComponent, FixtureDeferredConstructsComponent, FixtureElementSelectorComponent, FixtureFlatBindingsComponent, FixtureHydrateIdleConstructsComponent, FixtureLegacyStructuralConstructsComponent, FixtureLetBindingsComponent, FixtureModelTargetDirective, FixtureNamespacesComponent, FixturePostprocessedStructuralI18nComponent, FixturePrefetchIdleConstructsComponent, FixtureProjectionFallbackComponent, FixturePureBindingsComponent, FixturePureTargetDirective, FixtureSelectorMatrixComponent, FixtureStructuralConstructsComponent, FixtureStructuralI18nComponent, FixtureSubTemplateI18nComponent, FixtureTwoWayBindingComponent };
