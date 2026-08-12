import { Component } from '@angular/core';

@Component({
  selector: 'compat-card',
  standalone: true,
  template: `
    <button
      #primary
      type="button"
      [disabled]="disabled"
      [attr.aria-label]="label"
      [class.active]="active"
      [style.width.px]="width"
      (click)="select()"
    >
      {{ label }}
    </button>

    @if (visible) {
      <p>Visible</p>
      <button type="button" (click)="select()">Nested action</button>
    } @else {
      <p>Hidden</p>
    }

    <ul>
      @for (item of items; track item.id) {
        <li>{{ item.label }}</li>
      } @empty {
        <li>Empty</li>
      }
    </ul>
  `,
})
export class CompatibilityCardComponent {
  label = 'Angular 19';
  disabled = false;
  active = true;
  visible = true;
  width = 120;
  items = [{ id: 1, label: 'First' }];

  select() {
    this.active = !this.active;
  }
}

@Component({
  selector: 'compat-switch',
  standalone: true,
  template: `
    @switch (state()) {
      @case ('ready') {
        <strong>Ready</strong>
      }
      @default {
        <em>Idle</em>
      }
    }
  `,
})
export class CompatibilitySwitchComponent {
  state = () => 'ready';
}

@Component({
  selector: 'compat-icu',
  standalone: true,
  template: `
    <p i18n>
      {count, plural,
        =0 {No items}
        =1 {One item}
        other {{{ count }} items}
      }
    </p>
  `,
})
export class CompatibilityIcuComponent {
  count = 2;
}
