import { bootstrapApplication } from '@angular/platform-browser';
import { ɵɵconditionalCreate, ɵɵdefineComponent } from '@angular/core';
import { AppComponent } from './app/app.component';
import { FixtureCardComponent } from './app/fixture-card.component';
import { LazyCardComponent } from './app/lazy-card.component';

const producerGlobal = globalThis as typeof globalThis & {
  __wakaruAngularDefinitions?: unknown[];
  __wakaruAngularRoots?: unknown[];
  __wakaruIvyRuntime?: Record<string, unknown>;
  __wakaruStructuralRuntime?: unknown[];
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
  'ɵɵdefineComponent': ɵɵdefineComponent,
};
producerGlobal.__wakaruStructuralRuntime = [ɵɵconditionalCreate];

bootstrapApplication(AppComponent).catch((error) => console.error(error));
