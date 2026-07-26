import { Component } from '@angular/core';
import { FixtureCardComponent } from './fixture-card.component';

@Component({
  selector: 'app-root',
  imports: [FixtureCardComponent],
  template: `
    <main>
      <h1>{{ title }}</h1>
      <fixture-card />
    </main>
  `,
  styles: [':host { display: block; }'],
})
export class AppComponent {
  title = 'Angular production fixture';
}
