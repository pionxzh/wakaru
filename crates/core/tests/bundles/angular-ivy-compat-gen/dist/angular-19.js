/* Generated with Angular 19.2.25; see generate.mjs. */
import "@angular/core";
import * as i0 from "@angular/core";
const _forTrack0 = ($index, $item) => $item.id;
function CompatibilityCardComponent_Conditional_2_Template(rf, ctx) {
  rf & 1 && (i0.ɵɵelementStart(0, "p"), i0.ɵɵtext(1, "Visible"), i0.ɵɵelementEnd());
}
function CompatibilityCardComponent_Conditional_3_Template(rf, ctx) {
  rf & 1 && (i0.ɵɵelementStart(0, "p"), i0.ɵɵtext(1, "Hidden"), i0.ɵɵelementEnd());
}
function CompatibilityCardComponent_For_6_Template(rf, ctx) {
  if (rf & 1 && (i0.ɵɵelementStart(0, "li"), i0.ɵɵtext(1), i0.ɵɵelementEnd()), rf & 2) {
    const item_r1 = ctx.$implicit;
    i0.ɵɵadvance(), i0.ɵɵtextInterpolate(item_r1.label);
  }
}
function CompatibilityCardComponent_ForEmpty_7_Template(rf, ctx) {
  rf & 1 && (i0.ɵɵelementStart(0, "li"), i0.ɵɵtext(1, "Empty"), i0.ɵɵelementEnd());
}
function CompatibilitySwitchComponent_Case_0_Template(rf, ctx) {
  rf & 1 && (i0.ɵɵelementStart(0, "strong"), i0.ɵɵtext(1, "Ready"), i0.ɵɵelementEnd());
}
function CompatibilitySwitchComponent_Case_1_Template(rf, ctx) {
  rf & 1 && (i0.ɵɵelementStart(0, "em"), i0.ɵɵtext(1, "Idle"), i0.ɵɵelementEnd());
}
class CompatibilityCardComponent {
  label = "Angular 19";
  disabled = !1;
  active = !0;
  visible = !0;
  width = 120;
  items = [{ id: 1, label: "First" }];
  select() {
    this.active = !this.active;
  }
  static ɵfac = function(__ngFactoryType__) {
    return new (__ngFactoryType__ || CompatibilityCardComponent)();
  };
  static ɵcmp = /* @__PURE__ */ i0.ɵɵdefineComponent({ type: CompatibilityCardComponent, selectors: [["compat-card"]], decls: 8, vars: 9, consts: [["type", "button", 3, "click", "disabled"]], template: function(rf, ctx) {
    rf & 1 && (i0.ɵɵelementStart(0, "button", 0), i0.ɵɵlistener("click", function() {
      return ctx.select();
    }), i0.ɵɵtext(1), i0.ɵɵelementEnd(), i0.ɵɵtemplate(2, CompatibilityCardComponent_Conditional_2_Template, 2, 0, "p")(3, CompatibilityCardComponent_Conditional_3_Template, 2, 0, "p"), i0.ɵɵelementStart(4, "ul"), i0.ɵɵrepeaterCreate(5, CompatibilityCardComponent_For_6_Template, 2, 1, "li", null, _forTrack0, !1, CompatibilityCardComponent_ForEmpty_7_Template, 2, 0, "li"), i0.ɵɵelementEnd()), rf & 2 && (i0.ɵɵstyleProp("width", ctx.width, "px"), i0.ɵɵclassProp("active", ctx.active), i0.ɵɵproperty("disabled", ctx.disabled), i0.ɵɵattribute("aria-label", ctx.label), i0.ɵɵadvance(), i0.ɵɵtextInterpolate1(" ", ctx.label, " "), i0.ɵɵadvance(), i0.ɵɵconditional(ctx.visible ? 2 : 3), i0.ɵɵadvance(3), i0.ɵɵrepeater(ctx.items));
  }, encapsulation: 2 });
}
class CompatibilitySwitchComponent {
  state = () => "ready";
  static ɵfac = function(__ngFactoryType__) {
    return new (__ngFactoryType__ || CompatibilitySwitchComponent)();
  };
  static ɵcmp = /* @__PURE__ */ i0.ɵɵdefineComponent({ type: CompatibilitySwitchComponent, selectors: [["compat-switch"]], decls: 2, vars: 1, template: function(rf, ctx) {
    if (rf & 1 && i0.ɵɵtemplate(0, CompatibilitySwitchComponent_Case_0_Template, 2, 0, "strong")(1, CompatibilitySwitchComponent_Case_1_Template, 2, 0, "em"), rf & 2) {
      let tmp_0_0;
      i0.ɵɵconditional((tmp_0_0 = ctx.state()) === "ready" ? 0 : 1);
    }
  }, encapsulation: 2 });
}
export {
  CompatibilityCardComponent,
  CompatibilitySwitchComponent
};
