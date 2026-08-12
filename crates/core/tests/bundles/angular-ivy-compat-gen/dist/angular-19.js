/* Generated with Angular 19.2.25; see generate.mjs. */
import "@angular/core";
import * as i0 from "@angular/core";
const _forTrack0 = ($index, $item) => $item.id;
function CompatibilityCardComponent_Conditional_3_Template(rf, ctx) {
  if (rf & 1) {
    const _r2 = i0.ɵɵgetCurrentView();
    i0.ɵɵelementStart(0, "p"), i0.ɵɵtext(1, "Visible"), i0.ɵɵelementEnd(), i0.ɵɵelementStart(2, "button", 2), i0.ɵɵlistener("click", function() {
      i0.ɵɵrestoreView(_r2);
      const ctx_r2 = i0.ɵɵnextContext();
      return i0.ɵɵresetView(ctx_r2.select());
    }), i0.ɵɵtext(3, "Nested action"), i0.ɵɵelementEnd();
  }
}
function CompatibilityCardComponent_Conditional_4_Template(rf, ctx) {
  rf & 1 && (i0.ɵɵelementStart(0, "p"), i0.ɵɵtext(1, "Hidden"), i0.ɵɵelementEnd());
}
function CompatibilityCardComponent_For_7_Template(rf, ctx) {
  if (rf & 1 && (i0.ɵɵelementStart(0, "li"), i0.ɵɵtext(1), i0.ɵɵelementEnd()), rf & 2) {
    const item_r4 = ctx.$implicit;
    i0.ɵɵadvance(), i0.ɵɵtextInterpolate(item_r4.label);
  }
}
function CompatibilityCardComponent_ForEmpty_8_Template(rf, ctx) {
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
  static ɵcmp = /* @__PURE__ */ i0.ɵɵdefineComponent({ type: CompatibilityCardComponent, selectors: [["compat-card"]], decls: 9, vars: 9, consts: [["primary", ""], ["type", "button", 3, "click", "disabled"], ["type", "button", 3, "click"]], template: function(rf, ctx) {
    if (rf & 1) {
      const _r1 = i0.ɵɵgetCurrentView();
      i0.ɵɵelementStart(0, "button", 1, 0), i0.ɵɵlistener("click", function() {
        return i0.ɵɵrestoreView(_r1), i0.ɵɵresetView(ctx.select());
      }), i0.ɵɵtext(2), i0.ɵɵelementEnd(), i0.ɵɵtemplate(3, CompatibilityCardComponent_Conditional_3_Template, 4, 0)(4, CompatibilityCardComponent_Conditional_4_Template, 2, 0, "p"), i0.ɵɵelementStart(5, "ul"), i0.ɵɵrepeaterCreate(6, CompatibilityCardComponent_For_7_Template, 2, 1, "li", null, _forTrack0, !1, CompatibilityCardComponent_ForEmpty_8_Template, 2, 0, "li"), i0.ɵɵelementEnd();
    }
    rf & 2 && (i0.ɵɵstyleProp("width", ctx.width, "px"), i0.ɵɵclassProp("active", ctx.active), i0.ɵɵproperty("disabled", ctx.disabled), i0.ɵɵattribute("aria-label", ctx.label), i0.ɵɵadvance(2), i0.ɵɵtextInterpolate1(" ", ctx.label, " "), i0.ɵɵadvance(), i0.ɵɵconditional(ctx.visible ? 3 : 4), i0.ɵɵadvance(3), i0.ɵɵrepeater(ctx.items));
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
class CompatibilityIcuComponent {
  count = 2;
  static ɵfac = function(__ngFactoryType__) {
    return new (__ngFactoryType__ || CompatibilityIcuComponent)();
  };
  static ɵcmp = /* @__PURE__ */ i0.ɵɵdefineComponent({ type: CompatibilityIcuComponent, selectors: [["compat-icu"]], decls: 2, vars: 2, consts: () => {
    let i18n_0;
    typeof ngI18nClosureMode < "u" && ngI18nClosureMode ? i18n_0 = goog.getMsg("{VAR_PLURAL, plural, =0 {No items} =1 {One item} other {{INTERPOLATION} items}}") : i18n_0 = $localize`{VAR_PLURAL, plural, =0 {No items} =1 {One item} other {{INTERPOLATION} items}}`, i18n_0 = i0.ɵɵi18nPostprocess(i18n_0, { INTERPOLATION: "�1�", VAR_PLURAL: "�0�" });
    let i18n_1;
    return typeof ngI18nClosureMode < "u" && ngI18nClosureMode ? i18n_1 = goog.getMsg(" {$icu} ", { icu: i18n_0 }, { original_code: { icu: `{count, plural,
        =0 {No items}
        =1 {One item}
        other {{{ count }} items}
      }` } }) : i18n_1 = $localize` ${i18n_0}:ICU@@1118798863327598053: `, [i18n_1];
  }, template: function(rf, ctx) {
    rf & 1 && (i0.ɵɵelementStart(0, "p"), i0.ɵɵi18n(1, 0), i0.ɵɵelementEnd()), rf & 2 && (i0.ɵɵadvance(), i0.ɵɵi18nExp(ctx.count)(ctx.count), i0.ɵɵi18nApply(1));
  }, encapsulation: 2 });
}
export {
  CompatibilityCardComponent,
  CompatibilityIcuComponent,
  CompatibilitySwitchComponent
};
