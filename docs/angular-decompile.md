# Angular Ivy artifact recovery

Status: **experimental design and implementation guide**.

Wakaru recovers production Angular Ivy component definitions into readable
TypeScript inspection artifacts. The first output shape is one `.ts` artifact
per component with an inline `template` and inline `styles`:

```ts
import { Component } from "@angular/core";

@Component({
  selector: "demo-card",
  template: `
    <article>
      <h2>{{ title }}</h2>
    </article>
  `,
  styles: [`
    article {
      display: block;
    }
  `],
})
export class DemoCardComponent {
  title = "Example";
}
```

The artifact is for static inspection. It is not promised to compile or run
until every referenced class member, dependency, and import has been recovered.
The emitter must preserve uncertain expressions instead of inventing their
original names.

## Scope

The target is production AOT output. Development-only class metadata and a
copy of the original template are deliberately not inputs to recovery.

Initial recovery covers:

- component identity and selector;
- element, text, and static-attribute creation instructions;
- text interpolation;
- property, attribute, class, and style bindings;
- event listeners;
- inline component styles;
- the component class body after Ivy definition fields are removed.

Embedded views, modern control-flow blocks, projection, pipes, local template
references, and dependency/import reconstruction are incremental extensions.
Unsupported instruction regions remain explicit in the recovery IR and must
not be silently rendered as if recovery were complete.

## Architecture boundary

Bundle extraction and Ivy recovery are independent:

```text
ordinary JavaScript ─────────────────────────┐
                                            ▼
bundled JavaScript → format-specific unpack → module workspace
                                            │
                         ┌──────────────────┴─────────────────┐
                         ▼                                    ▼
                 readable JavaScript                  Ivy analyzer
                                                              │
                                                              ▼
                                              TypeScript component artifacts
```

An unpacker owns only transport concerns: bundle detection, module boundaries,
runtime parameters, dependency edges, and provenance. It never imports Angular
types or assigns Ivy meanings.

The module workspace exposes generic module identity, resolved ASTs, and symbol
edges. The Ivy analyzer owns:

- semantic instruction roles such as `DefineComponent`, `ElementStart`, and
  `TextInterpolate`;
- descriptor-field classification;
- component and template IR;
- TypeScript artifact emission.

The analyzer accepts both canonical instruction names and semantic role
evidence. One useful generic evidence source is an object literal whose stable
string keys name public exports while its values point to renamed local or
namespace symbols:

```js
const runtime = {
  "ɵɵdefineComponent": a,
  "ɵɵelementStart": b,
  "ɵɵelementEnd": c,
};
```

The workspace records `"ɵɵelementStart" → b`; it does not globally rename
`b`. The artifact emitter may print the canonical role name in diagnostics,
while normal JavaScript output keeps the actual binding. This prevents the Ivy
analyzer from depending on any one minifier or bundle format.

Descriptor property names may also be renamed. Recovery therefore prefers a
known canonical key when available, then classifies a value from structural
evidence:

- the template is a function that calls proven Ivy instruction bindings;
- selectors are nested string arrays with selector shape;
- styles are string arrays with stylesheet evidence;
- constant attribute tables are referenced by creation instructions.

Ambiguous values are left unknown. Object-literal order alone is not proof.

## Pipeline placement

Root recovery runs after the ordinary rewrite pipeline:

1. Finalize normal JavaScript without any Ivy-dependent rewrite.
2. Build one generic module workspace from all finalized modules.
3. Collect symbol-role evidence across the workspace.
4. Analyze component definitions using the shared role table.
5. Emit recovered artifacts independently from the JavaScript modules.

This placement lets a regular AOT module and modules obtained from any unpacker
use the same analyzer. The experimental implementation currently parses the
finalized owned workspace once. Retaining finalized ASTs through root artifact
recovery is a planned performance optimization; it must not alter the module
workspace contract or move Ivy semantics into an unpacker. Standalone
`angular::recover` is allowed to parse owned sources as a separate convenience
operation, matching the public API boundary documented in
[public-api.md](public-api.md).

The workspace may canonicalize a stable namespace argument passed into an
immediately invoked function when the corresponding parameter is never
reassigned. This is generic symbol-edge normalization. It neither identifies a
bundle format nor assigns an Ivy role; role classification remains in the Ivy
analyzer.

## Artifact contract

Root `decompile` and `unpack` operations use the framework-neutral artifact
model from [public-api.md](public-api.md). An Angular component currently emits
one TypeScript file, but the model remains multi-file so future source,
template, or stylesheet separation does not require another API break.

Artifact filenames are derived from the recovered component name or selector,
normalized as safe relative paths, and deduplicated across the complete module
workspace. Each artifact records the source module index.

Recovery confidence is structural, not a percentage:

- `Complete`: every emitted template region has a supported interpretation.
- `Partial`: the component is proven, but one or more regions are preserved as
  explicit unsupported output.

No artifact is emitted when component identity itself is ambiguous.

## Tests and local corpus policy

Committed tests use small synthetic production fixtures and generated,
redistributable Angular fixtures. Production fixtures must assert that they do
not contain development class metadata or the original template literal.

Private or proprietary bundles are local validation inputs only:

- store them under `.wakaru-local/`, which is ignored by Git;
- never copy their code into fixtures, snapshots, docs, diagnostics, or output
  examples;
- never commit identifying names, URLs, project identifiers, hashes, module
  labels, or provenance;
- report only aggregate recovery measurements when those measurements cannot
  identify the source.

The committed feature and its vocabulary remain generic Angular Ivy recovery.

## Architecture audit checkpoints

Pause and re-check this boundary after each milestone:

1. semantic-role and canonical-component detection;
2. inline template and class-body MVP;
3. renamed-instruction and renamed-descriptor support;
4. root operation and CLI artifact integration;
5. embedded views and control flow.

At each checkpoint verify that no unpacker contains Ivy roles, no Ivy module
branches on a bundle format, and no normal JavaScript rewrite depends on
artifact emission.
