import { UpperCasePipe } from '@angular/common';
import { Component } from '@angular/core';

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
