import { NgFor, NgIf, UpperCasePipe } from '@angular/common';
import { Component, Directive, model } from '@angular/core';

@Component({
  selector: 'fixture-flat-bindings',
  template: `
    <article
      title="Flat bindings"
      [attr.aria-label]="label"
      [class.active]="active"
      [style.opacity]="opacity"
      (click)="activate($event)"
    >
      <h2>{{ prefix }} {{ label }}</h2>
      <input [disabled]="disabled" />
    </article>
  `,
})
export class FixtureFlatBindingsComponent {
  label = 'Generated label';
  prefix = 'Status:';
  active = true;
  opacity = 0.75;
  disabled = false;

  activate(_event: MouseEvent) {
    this.active = !this.active;
  }
}

@Component({
  selector: 'fixture-container-i18n',
  template: `
    <section>
      <ng-container>
        <span>Grouped content</span>
      </ng-container>
      <p i18n>Hello, localized world!</p>
      <p i18n>Hello, {{ name }}!</p>
    </section>
  `,
})
export class FixtureContainerI18nComponent {
  name = 'reader';
}

@Component({
  selector: 'button[fixtureAction],a[fixtureAction]',
  template: `<span>Selector matrix</span>`,
})
export class FixtureSelectorMatrixComponent {}

@Component({
  selector: 'dialog[fixtureDialog]',
  template: `<span>Element selector</span>`,
})
export class FixtureElementSelectorComponent {}

@Directive({
  selector: 'fixture-pure-target',
  inputs: ['config', 'items'],
})
export class FixturePureTargetDirective {
  config: unknown;
  items: unknown;
}

@Component({
  selector: 'fixture-pure-bindings',
  imports: [FixturePureTargetDirective],
  template: `
    <fixture-pure-target [config]="{ fixed: true }" />
    <fixture-pure-target
      [config]="{ label: label }"
      [items]="[label]"
    />
    <button title="{{ label }}" attr.data-label="Hello {{ label }}!">
      Interpolate
    </button>
  `,
})
export class FixturePureBindingsComponent {
  label = 'reader';
}

@Component({
  selector: 'fixture-let-bindings',
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
export class FixtureLetBindingsComponent {
  prefix = 'Status: ';
  label = 'ready';
  active = true;

  activate(label: string) {
    console.log(label);
  }
}

@Component({
  selector: 'fixture-namespaces',
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
export class FixtureNamespacesComponent {}

@Component({
  selector: 'fixture-structural-constructs',
  imports: [UpperCasePipe],
  template: `
    <section>
      @if (showDetails) {
        <h2>{{ title | uppercase }}</h2>
      } @else {
        <p>Details hidden</p>
      }

      @for (item of items; track item.id) {
        <button #row type="button" (click)="select(row, item)">
          {{ item.label }}
        </button>
      } @empty {
        <p>No items</p>
      }

      <ng-content select="[card-footer]" />
    </section>
  `,
})
export class FixtureStructuralConstructsComponent {
  showDetails = true;
  title = 'Generated structures';
  items = [{ id: 1, label: 'First' }];

  select(_row: HTMLButtonElement, _item: { id: number; label: string }) {}
}

@Component({
  selector: 'fixture-complex-listener',
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
export class FixtureComplexListenerComponent {
  prefix = 'Selected: ';
  suffix = 'item';
  active = true;
  items = [{ id: 1, label: 'First' }];

  record(
    _button: HTMLButtonElement,
    _item: { id: number; label: string },
    _displayLabel: string,
  ) {
    console.log(_button, _item, _displayLabel);
  }
}

@Component({
  selector: 'fixture-structural-i18n',
  template: `
    <p i18n>Hello <strong>{{ name }}</strong>!</p>
  `,
})
export class FixtureStructuralI18nComponent {
  name = 'reader';
}

@Component({
  selector: 'fixture-projection-fallback',
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
export class FixtureProjectionFallbackComponent {}

@Directive({
  selector: 'fixture-model-target',
})
export class FixtureModelTargetDirective {
  value = model('');
}

@Component({
  selector: 'fixture-two-way-binding',
  imports: [FixtureModelTargetDirective],
  template: `
    <fixture-model-target [(value)]="name" />
  `,
})
export class FixtureTwoWayBindingComponent {
  name = 'reader';
}

@Component({
  selector: 'fixture-animation-bindings',
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
export class FixtureAnimationBindingsComponent {
  leaveClass = 'fade-out';

  started(_event: unknown) {
    console.log(_event);
  }
}

@Component({
  selector: 'fixture-deferred-constructs',
  template: `
    <section>
      @defer (on idle) {
        <article>{{ title }}</article>
      } @loading {
        <p>Loading</p>
      } @placeholder {
        <p>Waiting</p>
      } @error {
        <p>Failed</p>
      }
    </section>
  `,
})
export class FixtureDeferredConstructsComponent {
  title = 'Deferred content';
}

@Component({
  selector: 'fixture-prefetch-idle-constructs',
  template: `
    <section>
      @defer (on interaction; prefetch on idle) {
        <article>Prefetched content</article>
      } @placeholder {
        <button type="button">Load prefetched content</button>
      }
    </section>
  `,
})
export class FixturePrefetchIdleConstructsComponent {}

@Component({
  selector: 'fixture-hydrate-idle-constructs',
  template: `
    <section>
      @defer (on interaction; hydrate on idle) {
        <article>Hydrated content</article>
      } @placeholder {
        <button type="button">Load hydrated content</button>
      }
    </section>
  `,
})
export class FixtureHydrateIdleConstructsComponent {}

@Component({
  selector: 'fixture-legacy-structural-constructs',
  imports: [NgIf, NgFor],
  template: `
    <section>
      <p *ngIf="visible">Legacy visible</p>
      <span *ngFor="let item of items">{{ item }}</span>
    </section>
  `,
})
export class FixtureLegacyStructuralConstructsComponent {
  visible = true;
  items = ['First', 'Second'];
}
