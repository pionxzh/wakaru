import { bootstrapApplication } from '@angular/platform-browser';
import {
  Component,
  ɵɵconditionalCreate,
  ɵɵdefineComponent,
  ɵɵgetCurrentView,
  ɵɵnextContext,
  ɵɵresetView,
  ɵɵrestoreView,
} from '@angular/core';
import { AppComponent } from './app/app.component';
import { FixtureCardComponent } from './app/fixture-card.component';
import { LazyCardComponent } from './app/lazy-card.component';

const producerGlobal = globalThis as typeof globalThis & {
  __wakaruAngularDefinitions?: unknown[];
  __wakaruAngularRoots?: unknown[];
  __wakaruIvyRuntime?: Record<string, unknown>;
  __wakaruStructuralRuntime?: unknown[];
};

@Component({
  selector: 'structural-view-card',
  standalone: true,
  template: `
    @if (visible) {
      <button type="button" [disabled]="disabled" (click)="select()">
        Nested view
      </button>
    }
  `,
})
class StructuralViewCardComponent {
  visible = true;
  disabled = false;

  select(): void {
    this.disabled = true;
  }
}

producerGlobal.__wakaruAngularRoots = [
  AppComponent,
  FixtureCardComponent,
  LazyCardComponent,
  StructuralViewCardComponent,
];
producerGlobal.__wakaruAngularDefinitions = [
  (AppComponent as typeof AppComponent & { ɵcmp: unknown }).ɵcmp,
  (FixtureCardComponent as typeof FixtureCardComponent & { ɵcmp: unknown }).ɵcmp,
  (LazyCardComponent as typeof LazyCardComponent & { ɵcmp: unknown }).ɵcmp,
  (
    StructuralViewCardComponent as typeof StructuralViewCardComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
];
producerGlobal.__wakaruIvyRuntime = {
  'ɵɵdefineComponent': ɵɵdefineComponent,
};
producerGlobal.__wakaruStructuralRuntime = [
  ɵɵconditionalCreate,
  ɵɵgetCurrentView,
  ɵɵnextContext,
  ɵɵresetView,
  ɵɵrestoreView,
];

bootstrapApplication(AppComponent).catch((error) => console.error(error));
