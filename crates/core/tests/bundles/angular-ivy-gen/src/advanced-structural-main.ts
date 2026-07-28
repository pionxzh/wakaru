import { bootstrapApplication } from '@angular/platform-browser';
import {
  Component,
  ɵɵconditionalCreate,
  ɵɵdefer,
  ɵɵdeferOnIdle,
  ɵɵdefineComponent,
  ɵɵgetCurrentView,
  ɵɵnextContext,
  ɵɵrepeater,
  ɵɵrepeaterCreate,
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
    @for (item of items; track item.id) {
      <span>Item</span>
    } @empty {
      <span>No items</span>
    }
  `,
})
class StructuralViewCardComponent {
  visible = true;
  disabled = false;
  items = [{ id: 1, label: 'First' }];

  select(): void {
    this.disabled = true;
  }
}

@Component({
  selector: 'structural-defer-card',
  standalone: true,
  template: `
    @defer (on idle) {
      <article>{{ title }}</article>
    } @placeholder {
      <p>Waiting</p>
    }
  `,
})
class StructuralDeferCardComponent {
  title = 'Deferred content';
}

producerGlobal.__wakaruAngularRoots = [
  AppComponent,
  FixtureCardComponent,
  LazyCardComponent,
  StructuralViewCardComponent,
  StructuralDeferCardComponent,
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
  (
    StructuralDeferCardComponent as typeof StructuralDeferCardComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
];
producerGlobal.__wakaruIvyRuntime = {
  'ɵɵdefineComponent': ɵɵdefineComponent,
};
producerGlobal.__wakaruStructuralRuntime = [
  ɵɵconditionalCreate,
  ɵɵdefer,
  ɵɵdeferOnIdle,
  ɵɵgetCurrentView,
  ɵɵnextContext,
  ɵɵrepeater,
  ɵɵrepeaterCreate,
  ɵɵresetView,
  ɵɵrestoreView,
];

bootstrapApplication(AppComponent).catch((error) => console.error(error));
