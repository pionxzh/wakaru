import { Component } from '@angular/core';

@Component({
  selector: 'fixture-lazy-card',
  template: `<aside>Lazy {{ message }}</aside>`,
})
export class LazyCardComponent {
  message = 'chunk';
}
