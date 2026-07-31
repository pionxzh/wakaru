import { bootstrapApplication } from '@angular/platform-browser';
import {
  Component,
  Directive,
  ElementRef,
  computed,
  contentChild,
  contentChildren,
  inject,
  input,
  model,
  output,
  signal,
  viewChild,
  viewChildren,
  ɵɵanimateEnter,
  ɵɵanimateEnterListener,
  ɵɵanimateLeave,
  ɵɵanimateLeaveListener,
  ɵɵariaProperty,
  ɵɵattribute,
  ɵɵclassMap,
  ɵɵclassProp,
  ɵɵconditionalCreate,
  ɵɵdeclareLet,
  ɵɵdefer,
  ɵɵdeferHydrateOnIdle,
  ɵɵdeferOnIdle,
  ɵɵdeferPrefetchOnIdle,
  ɵɵelementContainer,
  ɵɵelementContainerEnd,
  ɵɵelementContainerStart,
  ɵɵdefineComponent,
  ɵɵgetCurrentView,
  ɵɵi18n,
  ɵɵi18nApply,
  ɵɵi18nEnd,
  ɵɵi18nExp,
  ɵɵi18nStart,
  ɵɵinterpolate,
  ɵɵinterpolate1,
  ɵɵnamespaceHTML,
  ɵɵnamespaceMathML,
  ɵɵnamespaceSVG,
  ɵɵnextContext,
  ɵɵpureFunction0,
  ɵɵpureFunction1,
  ɵɵreadContextLet,
  ɵɵrepeater,
  ɵɵrepeaterCreate,
  ɵɵresetView,
  ɵɵrestoreView,
  ɵɵstoreLet,
  ɵɵstyleMap,
  ɵɵstyleProp,
  ɵɵtextInterpolate2,
  ɵɵtwoWayBindingSet,
  ɵɵtwoWayListener,
  ɵɵtwoWayProperty,
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

@Directive({
  selector: '[structuralAriaTarget]',
  standalone: true,
})
class StructuralAriaTargetDirective {
  ariaLabel = input('', { alias: 'aria-label' });
}

@Component({
  selector: 'structural-binding-card',
  standalone: true,
  imports: [StructuralAriaTargetDirective],
  template: `
    <button
      #action
      type="button"
      [attr.aria-label]="label"
      [class]="classes"
      [class.active]="active"
      [style]="styles"
      [style.width.px]="width"
    >
      Bound content
    </button>
    <span structuralAriaTarget [aria-label]="label">{{ action.disabled }}</span>
  `,
})
class StructuralBindingCardComponent {
  label = 'Bound label';
  classes = 'bound primary';
  active = true;
  styles = 'color: rebeccapurple';
  width = 120;
}

@Component({
  selector: 'structural-container-i18n-card',
  standalone: true,
  template: `
    <ng-container>
      <span>Grouped content</span>
    </ng-container>
    <p i18n>Hello, localized world!</p>
    <p i18n>Hello, {{ name }}!</p>
  `,
})
class StructuralContainerI18nCardComponent {
  name = 'reader';
}

@Component({
  selector: 'button[fixtureAction],a[fixtureAction]',
  standalone: true,
  template: `<span>Selector matrix</span>`,
})
class StructuralSelectorMatrixComponent {}

@Component({
  selector: 'dialog[fixtureDialog]',
  standalone: true,
  template: `<span>Element selector</span>`,
})
class StructuralElementSelectorComponent {}

@Directive({
  selector: 'structural-pure-target',
  inputs: ['config', 'items'],
})
class StructuralPureTargetDirective {
  config: unknown;
  items: unknown;
}

@Component({
  selector: 'structural-pure-bindings',
  standalone: true,
  imports: [StructuralPureTargetDirective],
  template: `
    <structural-pure-target [config]="{ fixed: true }" />
    <structural-pure-target
      [config]="{ label: label }"
      [items]="[label]"
    />
    <button title="{{ label }}" attr.data-label="Hello {{ label }}!">
      Interpolate
    </button>
  `,
})
class StructuralPureBindingsComponent {
  label = 'reader';
}

@Component({
  selector: 'structural-let-bindings',
  standalone: true,
  template: `
    @let displayLabel = prefix + label;
    <p>{{ displayLabel }}</p>
    @if (active) {
      <button type="button" (click)="activate(displayLabel)">
        {{ displayLabel }}
      </button>
    }
  `,
})
class StructuralLetBindingsComponent {
  prefix = 'Status: ';
  label = 'ready';
  active = true;

  activate(label: string): void {
    console.log(label);
  }
}

@Component({
  selector: 'structural-complex-listener',
  standalone: true,
  template: `
    @let displayLabel = prefix + suffix;
    @for (item of items; track item.id) {
      @if (active) {
        <button
          #button
          type="button"
          (click)="record(button, item, displayLabel); active = false"
        >
          {{ item.label }}
        </button>
      }
    }
  `,
})
class StructuralComplexListenerComponent {
  prefix = 'Selected: ';
  suffix = 'item';
  active = true;
  items = [{ id: 1, label: 'First' }];

  record(
    _button: HTMLButtonElement,
    _item: { id: number; label: string },
    _displayLabel: string,
  ): void {
    const payload = {
      button: _button,
      item: _item,
      label: _displayLabel,
    };
    console.log(payload);
    if (_item.label) {
      payload.label = _item.label;
    }
    console.log(payload);
  }
}

@Component({
  selector: 'structural-i18n-region',
  standalone: true,
  template: `
    <p i18n>Hello <strong>{{ name }}</strong>!</p>
  `,
})
class StructuralI18nRegionComponent {
  name = 'reader';
}

@Component({
  selector: 'structural-projection-fallback',
  standalone: true,
  template: `
    <section>
      <ng-content select="[card-title]">
        <h2>Fallback title</h2>
      </ng-content>
      <ng-content>
        <p>Fallback body</p>
      </ng-content>
    </section>
  `,
})
class StructuralProjectionFallbackComponent {}

@Directive({
  selector: 'structural-model-target',
})
class StructuralModelTargetDirective {
  value = model('');
}

@Component({
  selector: 'structural-two-way-binding',
  standalone: true,
  imports: [StructuralModelTargetDirective],
  template: `
    <structural-model-target [(value)]="name" />
  `,
})
class StructuralTwoWayBindingComponent {
  name = 'reader';
}

@Component({
  selector: 'structural-animation-bindings',
  standalone: true,
  template: `
    <div
      animate.enter="fade-in"
      [animate.leave]="leaveClass"
      (animate.enter)="started($event)"
    >
      Animated
    </div>
  `,
})
class StructuralAnimationBindingsComponent {
  leaveClass = 'fade-out';

  started(_event: unknown): void {
    console.log(_event);
  }
}

@Component({
  selector: 'structural-namespaces',
  standalone: true,
  template: `
    <svg viewBox="0 0 10 10">
      <circle cx="5" cy="5" r="4" />
    </svg>
    <math>
      <mi>x</mi><mo>=</mo><mn>1</mn>
    </math>
    <p>HTML</p>
  `,
})
class StructuralNamespacesComponent {}

class StructuralApiService {}

@Component({
  selector: 'structural-class-apis',
  standalone: true,
  template: `
    <button type="button" (click)="increment()">
      {{ label() }}: {{ count() }}
    </button>
    <button type="button" (click)="changed.emit()">Notify</button>
  `,
})
class StructuralClassApisComponent {
  name = input('reader');
  count = signal(0);
  label = computed(() => this.name().toUpperCase());
  service = inject(StructuralApiService, { optional: true });
  selection = model('');
  changed = output<void>();

  increment(): void {
    this.count.update((value) => value + 1);
  }
}

@Component({
  selector: 'structural-query-apis',
  standalone: true,
  imports: [StructuralClassApisComponent],
  template: `
    <div #viewOptional>Optional view child</div>
    <div #viewRequired>Required view child</div>
    <div #viewMany>First view child</div>
    <div #viewMany>Second view child</div>
    <structural-class-apis />
    <label
      (click)="$event.target !== optionalTarget()?.nativeElement && handleOptionalClick()"
    >
      Optional listener
    </label>
    <span>{{ optionalClicks }}</span>
    <ng-content />
  `,
})
class StructuralQueryApisComponent {
  optionalTarget = viewChild<ElementRef<HTMLDivElement>>('viewOptional');
  viewRequired = viewChild.required('viewRequired');
  viewMany = viewChildren('viewMany', { read: ElementRef });
  viewType = viewChild(StructuralClassApisComponent);
  contentOptional = contentChild('contentOptional', { descendants: false });
  contentRequired = contentChild.required('contentRequired', {
    read: ElementRef,
  });
  contentMany = contentChildren('contentMany');
  optionalClicks = 0;

  handleOptionalClick(): number {
    return ++this.optionalClicks;
  }
}

producerGlobal.__wakaruAngularRoots = [
  AppComponent,
  FixtureCardComponent,
  LazyCardComponent,
  StructuralViewCardComponent,
  StructuralDeferCardComponent,
  StructuralPrefetchIdleCardComponent,
  StructuralHydrateIdleCardComponent,
  StructuralBindingCardComponent,
  StructuralContainerI18nCardComponent,
  StructuralSelectorMatrixComponent,
  StructuralElementSelectorComponent,
  StructuralPureBindingsComponent,
  StructuralLetBindingsComponent,
  StructuralComplexListenerComponent,
  StructuralI18nRegionComponent,
  StructuralProjectionFallbackComponent,
  StructuralTwoWayBindingComponent,
  StructuralAnimationBindingsComponent,
  StructuralNamespacesComponent,
  StructuralClassApisComponent,
  StructuralQueryApisComponent,
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
  (
    StructuralBindingCardComponent as typeof StructuralBindingCardComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralContainerI18nCardComponent as typeof StructuralContainerI18nCardComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralSelectorMatrixComponent as typeof StructuralSelectorMatrixComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralElementSelectorComponent as typeof StructuralElementSelectorComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralPureBindingsComponent as typeof StructuralPureBindingsComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralLetBindingsComponent as typeof StructuralLetBindingsComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralComplexListenerComponent as typeof StructuralComplexListenerComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralI18nRegionComponent as typeof StructuralI18nRegionComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralProjectionFallbackComponent as typeof StructuralProjectionFallbackComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralTwoWayBindingComponent as typeof StructuralTwoWayBindingComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralAnimationBindingsComponent as typeof StructuralAnimationBindingsComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralNamespacesComponent as typeof StructuralNamespacesComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralClassApisComponent as typeof StructuralClassApisComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
  (
    StructuralQueryApisComponent as typeof StructuralQueryApisComponent & {
      ɵcmp: unknown;
    }
  ).ɵcmp,
];
producerGlobal.__wakaruIvyRuntime = {
  'ɵɵdefineComponent': ɵɵdefineComponent,
};
producerGlobal.__wakaruStructuralRuntime = [
  ɵɵanimateEnter,
  ɵɵanimateEnterListener,
  ɵɵanimateLeave,
  ɵɵanimateLeaveListener,
  ɵɵariaProperty,
  ɵɵattribute,
  ɵɵclassMap,
  ɵɵclassProp,
  ɵɵconditionalCreate,
  ɵɵdeclareLet,
  ɵɵdefer,
  ɵɵdeferHydrateOnIdle,
  ɵɵdeferOnIdle,
  ɵɵdeferPrefetchOnIdle,
  ɵɵelementContainer,
  ɵɵelementContainerEnd,
  ɵɵelementContainerStart,
  ɵɵgetCurrentView,
  ɵɵi18n,
  ɵɵi18nApply,
  ɵɵi18nEnd,
  ɵɵi18nExp,
  ɵɵi18nStart,
  ɵɵinterpolate,
  ɵɵinterpolate1,
  ɵɵnamespaceHTML,
  ɵɵnamespaceMathML,
  ɵɵnamespaceSVG,
  ɵɵnextContext,
  ɵɵpureFunction0,
  ɵɵpureFunction1,
  ɵɵreadContextLet,
  ɵɵrepeater,
  ɵɵrepeaterCreate,
  ɵɵresetView,
  ɵɵrestoreView,
  ɵɵstoreLet,
  ɵɵstyleMap,
  ɵɵstyleProp,
  ɵɵtextInterpolate2,
  ɵɵtwoWayBindingSet,
  ɵɵtwoWayListener,
  ɵɵtwoWayProperty,
];

bootstrapApplication(AppComponent).catch((error) => console.error(error));
