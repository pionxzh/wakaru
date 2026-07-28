import { UpperCasePipe } from "@angular/common";
import "@angular/core";
import * as i0 from "@angular/core";
var FixtureStructuralConstructsComponent_Conditional_1_Template, FixtureStructuralConstructsComponent_Conditional_2_Template, FixtureStructuralConstructsComponent_For_4_Template, FixtureStructuralConstructsComponent_ForEmpty_5_Template, FixtureDeferredConstructsComponent_Defer_1_Template, FixtureDeferredConstructsComponent_DeferLoading_2_Template, FixtureDeferredConstructsComponent_DeferPlaceholder_3_Template, FixtureDeferredConstructsComponent_DeferError_4_Template;
const _c0 = [[["", "card-footer", ""]]], _c1 = ["[card-footer]"], _forTrack0 = ($index, $item) => $item.id;
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
class FixtureStructuralConstructsComponent {
    showDetails = !0;
    title = "Generated structures";
    items = [{ id: 1, label: "First" }];
    select(_row, _item) {
    }
    static \u0275fac = function (__ngFactoryType__) {
        return new (__ngFactoryType__ || FixtureStructuralConstructsComponent)();
    };
    static \u0275cmp = i0.\u0275\u0275defineComponent({ type: FixtureStructuralConstructsComponent, selectors: [["fixture-structural-constructs"]], ngContentSelectors: _c1, decls: 7, vars: 2, consts: [["row", ""], ["type", "button"], ["type", "button", 3, "click"]], template: function (rf, ctx) {
            rf & 1 && (i0.\u0275\u0275projectionDef(_c0), i0.\u0275\u0275domElementStart(0, "section"), i0.\u0275\u0275conditionalCreate(1, FixtureStructuralConstructsComponent_Conditional_1_Template, 3, 3, "h2")(2, FixtureStructuralConstructsComponent_Conditional_2_Template, 2, 0, "p"), i0.\u0275\u0275repeaterCreate(3, FixtureStructuralConstructsComponent_For_4_Template, 3, 1, "button", 1, _forTrack0, !1, FixtureStructuralConstructsComponent_ForEmpty_5_Template, 2, 0, "p"), i0.\u0275\u0275projection(6), i0.\u0275\u0275domElementEnd()), rf & 2 && (i0.\u0275\u0275advance(), i0.\u0275\u0275conditional(ctx.showDetails ? 1 : 2), i0.\u0275\u0275advance(2), i0.\u0275\u0275repeater(ctx.items));
        }, dependencies: [UpperCasePipe], encapsulation: 2 });
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
export { FixtureDeferredConstructsComponent, FixtureFlatBindingsComponent, FixtureStructuralConstructsComponent };
