import { bootstrapApplication } from '@angular/platform-browser';
import { AppComponent } from './app/app.component';

bootstrapApplication(AppComponent)
  .then(() => import('./app/lazy-card.component'))
  .catch((error) => console.error(error));
