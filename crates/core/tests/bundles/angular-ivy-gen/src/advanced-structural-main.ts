import { bootstrapApplication } from '@angular/platform-browser';
import {
  Component,
  ɵɵconditionalCreate,
  ɵɵdefer,
  ɵɵdeferHydrateOnIdle,
  ɵɵdeferOnIdle,
  ɵɵdeferPrefetchOnIdle,
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

@Component({
  selector: 'structural-prefetch-idle-card',
  standalone: true,
  template: `
    @defer (on interaction; prefetch on idle) {
      <article>Prefetched content</article>
    } @placeholder {
      <button type="button">Load prefetched content</button>
    }
  `,
})
class StructuralPrefetchIdleCardComponent {}

@Component({
  selector: 'structural-hydrate-idle-card',
  standalone: true,
  template: `
    @defer (on interaction; hydrate on idle) {
      <article>Hydrated content</article>
    } @placeholder {
      <button type="button">Load hydrated content</button>
    }
  `,
})
class StructuralHydrateIdleCardComponent {}

producerGlobal.__wakaruAngularRoots = [
  AppComponent,
  FixtureCardComponent,
  LazyCardComponent,
  StructuralViewCardComponent,
  StructuralDeferCardComponent,
  StructuralPrefetchIdleCardComponent,
  StructuralHydrateIdleCardComponent,
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
    StructuralPrefetchIdleCardComponent as typeof StructuralPrefetchIdleCardComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralHydrateIdleCardComponent as typeof StructuralHydrateIdleCardComponent & {
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
  ɵɵdeferHydrateOnIdle,
  ɵɵdeferOnIdle,
  ɵɵdeferPrefetchOnIdle,
  ɵɵgetCurrentView,
  ɵɵnextContext,
  ɵɵrepeater,
  ɵɵrepeaterCreate,
  ɵɵresetView,
  ɵɵrestoreView,
];

bootstrapApplication(AppComponent).catch((error) => console.error(error));
