import { Component } from '@angular/core';

@Component({
  selector: 'compat-card',
  standalone: true,
  template: `
    <button
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
