import { bootstrapApplication } from '@angular/platform-browser';
import {
  ɵɵadvance,
  ɵɵconditional,
  ɵɵconditionalBranchCreate,
  ɵɵconditionalCreate,
  ɵɵdefineComponent,
  ɵɵdomElement,
  ɵɵdomElementEnd,
  ɵɵdomElementStart,
  ɵɵdomListener,
  ɵɵdomProperty,
  ɵɵdomTemplate,
  ɵɵelement,
  ɵɵelementEnd,
  ɵɵelementStart,
  ɵɵlistener,
  ɵɵnextContext,
  ɵɵpipe,
  ɵɵpipeBind1,
  ɵɵprojection,
  ɵɵprojectionDef,
  ɵɵproperty,
  ɵɵreference,
  ɵɵtemplate,
  ɵɵtext,
  ɵɵtextInterpolate,
  ɵɵtextInterpolate1,
} from '@angular/core';
import { AppComponent } from './app/app.component';
import { FixtureCardComponent } from './app/fixture-card.component';
import { LazyCardComponent } from './app/lazy-card.component';

const producerGlobal = globalThis as typeof globalThis & {
  __wakaruAngularDefinitions?: unknown[];
  __wakaruAngularRoots?: unknown[];
  __wakaruIvyRuntime?: Record<string, unknown>;
};

producerGlobal.__wakaruAngularRoots = [
  AppComponent,
  FixtureCardComponent,
  LazyCardComponent,
];
producerGlobal.__wakaruAngularDefinitions = [
  (AppComponent as typeof AppComponent & { ɵcmp: unknown }).ɵcmp,
  (FixtureCardComponent as typeof FixtureCardComponent & { ɵcmp: unknown }).ɵcmp,
  (LazyCardComponent as typeof LazyCardComponent & { ɵcmp: unknown }).ɵcmp,
];
producerGlobal.__wakaruIvyRuntime = {
  'ɵɵadvance': ɵɵadvance,
  'ɵɵconditional': ɵɵconditional,
  'ɵɵconditionalBranchCreate': ɵɵconditionalBranchCreate,
  'ɵɵconditionalCreate': ɵɵconditionalCreate,
  'ɵɵdefineComponent': ɵɵdefineComponent,
  'ɵɵdomElement': ɵɵdomElement,
  'ɵɵdomElementEnd': ɵɵdomElementEnd,
  'ɵɵdomElementStart': ɵɵdomElementStart,
  'ɵɵdomListener': ɵɵdomListener,
  'ɵɵdomProperty': ɵɵdomProperty,
  'ɵɵdomTemplate': ɵɵdomTemplate,
  'ɵɵelement': ɵɵelement,
  'ɵɵelementEnd': ɵɵelementEnd,
  'ɵɵelementStart': ɵɵelementStart,
  'ɵɵlistener': ɵɵlistener,
  'ɵɵnextContext': ɵɵnextContext,
  'ɵɵpipe': ɵɵpipe,
  'ɵɵpipeBind1': ɵɵpipeBind1,
  'ɵɵprojection': ɵɵprojection,
  'ɵɵprojectionDef': ɵɵprojectionDef,
  'ɵɵproperty': ɵɵproperty,
  'ɵɵreference': ɵɵreference,
  'ɵɵtemplate': ɵɵtemplate,
  'ɵɵtext': ɵɵtext,
  'ɵɵtextInterpolate': ɵɵtextInterpolate,
  'ɵɵtextInterpolate1': ɵɵtextInterpolate1,
};

bootstrapApplication(AppComponent).catch((error) => console.error(error));
