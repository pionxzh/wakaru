# Angular Ivy compatibility fixture

This independently pinned fixture verifies ordinary production AOT recovery
against Angular 19.2.25 while the primary `angular-ivy-gen` fixture tracks
Angular 22 and Closure Compiler behavior. Keeping the producers separate avoids
mixing regular Angular version compatibility with Closure-specific structural
inference.

The components cover a listener, property/attribute/class/style bindings,
interpolation, `@if` / `@else`, `@for` / `@empty`, and the scratch binding
generated for `@switch`. Their committed output is full-AOT JavaScript with
development metadata and the authored templates removed.

Regenerate and verify it with:

```bash
npm ci
npm run generate
npm run check
```
