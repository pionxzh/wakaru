# Angular Ivy artifact recovery

Status: **experimental design and implementation guide**.

Wakaru recovers production Angular Ivy component definitions into readable
TypeScript inspection artifacts. The output unit is one `.angular.ts` artifact
per recovered source module. Sibling components stay together, with each
component retaining its inline `template` and inline `styles`:

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

- component identity and element/attribute selector matrices;
- element, text, `<ng-container>`, and static-attribute creation instructions,
  including HTML, SVG, and MathML namespace transitions;
- text interpolation;
- property, attribute, class, and style bindings, including expression
  interpolation and compiler-hoisted pure object/array literals;
- event listeners;
- `@let` declarations and reads across nested views;
- nested embedded views selected by modern `@if` / `@else if` / `@else`
  control flow, including component-context reads through `ɵɵnextContext`;
- modern `@for` / `@empty` repeaters when the creation/update pair and track
  function are proven, including `$implicit` loop bindings and captured-view
  listeners that use `ɵɵrestoreView` / `ɵɵresetView`;
- bounded `@defer` blocks with primary, loading, placeholder, and error views,
  plus an immediately following `on idle` trigger;
- legacy structural-directive templates in neutral `<ng-template>` form when
  their ordinary template and property operations are recoverable;
- content projection, including selector-bearing `<ng-content>` slots;
- local template references and their use in binding expressions;
- declared pipes and fixed- or variadic-argument pipe bindings;
- bounded static and interpolated i18n messages;
- exact ESM imports referenced by the recovered class or rendered template,
  plus dependency-closed portable local helpers;
- compiled component dependencies when their bindings can be materialized as
  imports or portable local aliases;
- inline component styles;
- the component class body after Ivy definition fields are removed.

Other defer triggers, defer timing/hydration metadata, legacy
structural-directive re-sugaring, and original package provenance after
bundling remain incremental extensions. Dependencies between recovered
siblings in the same module are linked by binding identity; ordinary ESM edges
can also link recovered components in different artifacts. Unsupported
instruction regions remain explicit in the recovery IR and must not be
silently rendered as if recovery were complete.

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
                                                TypeScript module artifacts
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

For ordinary production chunk sets, the workspace also records direct
default, named, and namespace ESM import/export equivalences across resolved
relative filenames. Those edges carry binding identity only. The Ivy role
table projects semantic evidence across an equivalence group afterward, and a
conflicting role makes the whole group ambiguous rather than selecting one
side. This is the same generic workspace operation regardless of how a module
was produced.

Descriptor property names may also be renamed. Recovery therefore prefers a
known canonical key when available, then classifies a value from structural
evidence:

- the template is a function that calls proven Ivy instruction bindings;
- selectors are nested string arrays with selector shape;
- styles are string arrays with stylesheet evidence;
- constant attribute tables are referenced by creation instructions.

Ambiguous values are left unknown. Object-literal order alone is not proof.

## Pipeline placement

Root recovery uses two views of the same generic module set:

1. After format-specific extraction and numeric-edge normalization, capture a
   pre-rewrite evidence view only when Angular recovery is requested.
2. Run the ordinary Wakaru pipeline and finalize normal JavaScript without an
   Ivy-dependent rewrite.
3. Build the Ivy role table and component/template IR from the evidence view.
4. Match each proven component to a unique finalized class binding in the same
   module. Use that readable class body when the match is unambiguous; otherwise
   retain the evidence class.
5. Emit artifacts independently from the JavaScript modules.

This split is required because an ordinary readability rule can erase useful
compiler evidence. For example, `ObjectAssignSpread` changes a descriptor
builder from `Object.assign({}, base, descriptor)` into an object spread. The
result is better JavaScript, but no longer proves the same structural
`DefineComponent` role to a matcher that intentionally recognizes the producer
shape. Capturing evidence once avoids teaching the Ivy analyzer every shape
created by every later Wakaru rule.

The evidence sidecar is generic source keyed by the final module filename. The
driver does not import Angular types or assign roles, and every unpacker still
uses the same path. The current implementation materializes and reparses the
two views only when recovery is enabled. The independent evidence and readable
parses run in parallel while sharing one SWC `Globals` identity domain; this
keeps `SyntaxContext` values distinct across the complete module workspace.
Parser source maps are not retained after resolution because artifact printing
uses the recovered AST rather than source-position lookups. Retaining resolved
ASTs and a stable cross-stage origin ID would remove the remaining reparse cost
later, but would require a larger cross-stage ownership contract.

Standalone recovery receives one source view and therefore parses it once,
matching the convenience API boundary documented in [public-api.md](public-api.md).

Structural runtime functions are collected once for both initial Ivy-role
inference and template-use inference. After cross-module aliases are installed,
an equivalence index resolves a runtime binding to its unique function in
constant expected time. This avoids rescanning every runtime function for each
template call while preserving the existing fail-closed behavior for duplicate
or ambiguous definitions.

Embedded-view function identity is likewise binding-based. Besides function
declarations and initialized function variables, the module table accepts a
predeclared local with exactly one direct assignment to a function, plus stable
identifier aliases of that binding. This covers Closure ModuleManager-style
`var view; view = function (...) { ... }` output without treating an arbitrary
or reassigned runtime value as a template function.

Closure `ADVANCED` can also turn the first embedded-template instruction into
a wrapper that returns a separate self-returning continuation used by chained
calls. The Ivy classifier treats both functions as the same template role only
when calls in creation blocks have valid embedded-template arguments, the
continuation has the expected eight-parameter family shape, and both functions
forward their parameter dependencies in order to the same internal callee.
Matching arity or a returned function alone is insufficient.

Optimized control-flow roles use related runtime and template-use evidence:

- a Closure-renamed repeater creator must have Angular's thirteen-parameter
  runtime family, retain the `NgControlFlow` marker, and be called in creation
  blocks with a valid seven-to-thirteen-argument template shape;
- its update helper must be uniquely paired in those same views, take the
  collection in update phase, retain the `try`/`finally` consumer guard and
  selected-index state access, and retain a nontrivial direct-call body;
- identity/index track helpers are accepted only when their complete body
  returns the corresponding parameter;
- a Closure-renamed defer creator must retain the `NgDefer` runtime marker and
  reference child-template slots already declared in the same view;
- the idle-trigger role is inferred only inside a proven defer view from its
  timeout-object runtime behavior.

Rendering remains stricter than role classification. A defer block consumes
only unattributed, unbranched child templates that are the immediate trailing
siblings in declaration order. Timing/hydration arguments and non-idle
triggers remain explicit partial regions. Repeater context aliases are restored
from canonical `$implicit`/`$...` members in either declaration or assignment
form. If Closure renamed those context properties, Wakaru does not guess
whether a field meant `$implicit`, `$index`, or another contextual value.

Template-use evidence is semantic rather than tied to one minifier statement
shape. A fluent property instruction may appear as either an effect or an
initializer after Closure rewriting, and a conditional may select a fixed
template index directly instead of using a ternary. Those forms can corroborate
a role, but they do not replace runtime-body proof. Generic property helpers
must forward the name, value, and sanitizer in order to the renderer path;
specialized helpers must forward the name and value in order to a direct
`setProperty` call. A direct renderer call with those operands reversed remains
unknown.

Closure may preserve the view-state helpers while inlining
`ɵɵgetCurrentView()` into a direct member read. The classifier identifies
`ɵɵrestoreView` and `ɵɵresetView` only as a unique pair that writes the same
state member: the restore helper assigns its parameter and returns that
parameter's context slot, while the reset helper assigns `null` and returns its
parameter unchanged. `ɵɵnextContext` additionally requires proven initializer
use in an embedded view's update phase, its numeric depth default, and the
compiled parent/context slot traversal. A member-valued creation-phase
declaration is treated as an inlined current-view capture only when the same
binding is later passed to a proven restore helper inside that view. The member
shape alone is not semantic evidence, and ambiguous helper pairs remain
unknown. When the getter call survives, its zero-argument function must return
a member of the same state object and the captured result must flow to the
proven restore helper before it receives the `ɵɵgetCurrentView` role.

Closure can similarly specialize `ɵɵreference(slot)` around a view-slot read.
The specialized form is accepted only with proven update-phase initializer
uses, the Angular `27 + slot` layout relation, a returned checked slot, and the
sentinel-error branch retained by the runtime helper. A bare
`state[27 + slot]` loader is not enough: Angular uses the same primitive for
non-reference slots, so it remains unknown without a stronger relationship.
Resolving a proven reference slot to a template name is still view-local and
requires a matching creation-table declaration.

Constant-table recovery accepts either a direct component-local array or a
unique factory whose complete return behavior proves that it produces the
array. Entries are resolved independently, so one opaque value does not discard
otherwise proven static attributes or local-reference declarations. The
special case `"a;b".split(";")` is decoded only when both operands are exact
string literals with one nonempty ASCII delimiter. Arbitrary calls are never
evaluated. Reference declarations are collected for the complete view before
child-view and update recovery, allowing an embedded view to use a reference
declared later in its parent creation block without flattening slot scopes.

The `@let` family is recovered only when declaration, store, and context-read
roles satisfy their respective runtime and template-use contracts. A
Closure-renamed store helper must forward one value into a multi-argument view
write, return that same value, and be used in update phase; call count or arity
alone is insufficient. Read slots remain view-local. When compilation or
minification preserves a readable local binding, the artifact uses it. When
Closure has erased the authored name, Wakaru emits a deterministic neutral name
such as `value` rather than claiming to know the original spelling.

Namespace helpers are inferred as a family: zero-argument creation helpers must
write `svg`, `math`/`mathml`, and/or `null` to the same proven runtime state
member. A surviving HTML helper or Closure-inlined `state.namespace = null`
assignment is consumed only against that exact target. Namespace operations do
not produce standalone template syntax; their effect is represented by the
recovered `<svg>`, `<math>`, and following HTML elements.

Expression interpolation roles are paired with the already-proven text
interpolation family and their exact parameter-forwarding behavior. Pure
function bindings are expanded only when the callback and value arity are
proven and the callback body can be substituted safely. Otherwise the runtime
operation remains explicit unsupported output. Basic i18n recovery is
similarly bounded to a uniquely resolved static/interpolated message and a
valid containing element; structural ICU regions and ambiguous message
factories remain partial.

Each recovered render function owns its node cursor, reference slots, context
depth, aliases, and listener operations. Entering an embedded view snapshots
the already-proven ancestor reference scopes rather than flattening their
numeric indexes into the child. An update- or listener-phase
`ɵɵnextContext(depth)` advances only that view's context cursor; a following
`ɵɵreference(slot)` resolves against the exact ancestor depth or produces a
missing-target issue. This also lets restored listeners retain ordered ordinary
effects, substitute proven context-member aliases, and treat a zero-argument
`ɵɵresetView()` return as plumbing. Calls on an unresolved runtime namespace
are never substituted into the event expression.

### Performance and profiling

`--profile` exposes these top-level Angular spans:

- `angular: prepare modules` or `angular: prepare module views`;
- `angular: recover prepared modules`;
- `angular: infer Ivy roles`;
- `angular: index artifact symbols`;
- `angular: recover components`;
- `angular: link module artifacts`.

The nested spans distinguish unavoidable parsing from semantic analysis and
artifact rendering. A local reference run on a 12.3 MB, 239-module production
corpus used a release CLI, one warmup, and five measured runs. Normal unpack
averaged 6.22 seconds (5.82–7.10 seconds). After optimized repeater/defer role
inference was added, Angular unpack averaged 7.20 seconds (6.94–7.45 seconds).
An earlier release trace spent about 259 ms preparing the two views and
1.10 seconds in prepared-module recovery, including 265 ms for role inference,
36 ms for artifact-symbol indexing, 704 ms for component recovery, and 49 ms
linking recovered modules. The final 132 module-oriented sidecars account for
part of the remaining CLI difference. Neither the link step nor the new
structural scans currently justify a specialized optimization. These figures
validate the current implementation; they are not a performance contract
across machines or corpora.

The workspace may canonicalize a stable namespace argument passed into an
immediately invoked function when the corresponding parameter is never
reassigned. This is generic symbol-edge normalization. It neither identifies a
bundle format nor assigns an Ivy role; role classification remains in the Ivy
analyzer.

## Artifact contract

Root `decompile` and `unpack` operations use the framework-neutral artifact
model from [public-api.md](public-api.md). All recovered components originating
from one source module emit as one TypeScript inspection artifact. The
low-level report still exposes per-component results and diagnostics for
callers that need them, plus module results that link back to those component
indices.

Artifact filenames are derived from the source-module path as
`<module-stem>.angular.ts`, normalized as safe relative paths, and deduplicated
across the complete module workspace. Each artifact records the source module
index. Recovered class names are also unique within an artifact: when distinct
bindings infer the same readable name, later siblings receive a deterministic
numeric suffix.

Recovery confidence is structural, not a percentage:

- `Complete`: every render-phase operation observed by the analyzer was
  rendered successfully, all instruction arguments and target nodes were
  valid, and no statement or expression shape was skipped.
- `Partial`: the component is proven, but one or more regions are preserved as
  explicit unsupported output.

Recovery is fail-closed. Unsupported statements, expressions, instruction
roles, malformed arguments, missing target nodes, and malformed element
structure are typed issues rather than silent omissions. The core analysis API
returns every issue occurrence rather than deduplicating equal reasons. Each
issue identifies its source-module index and component, deterministic
depth-first view, render phase, per-view operation ordinal, and module-relative
byte range. A known Ivy operation records both its canonical role and the
concise callee spelling observed in the compiled input. Ranges refer to the
evidence source when the two-view API is used. Repeated HTML warning comments
remain deduplicated for readability; that display choice never erases analysis
locations or counts. The template IR places a warning at its proven
creation/update anchor when one exists. Otherwise it emits the warning inside
the smallest affected view with an explicit `placement unknown within this
view` marker; nested-view failures are no longer appended to the root template.

The same report includes instruction-call accounting plus workspace totals for
component candidates, rejected descriptors, complete/partial artifacts,
rendered calls, unsupported calls, and malformed calls. Root operations surface
the aggregate report when diagnostics are enabled. Unknown runtime calls are
grouped by render phase and invocation arity; these privacy-safe shapes help
prioritize compiler-pattern support without depending on Closure-renamed
identifiers. Diagnostics retain only callee spellings, arities, indexes,
reasons, and source coordinates; they never copy raw argument source.

No artifact is emitted when component identity itself is ambiguous.

## Artifact-local symbols and dependencies

Artifact support recovery is a separate, binding-identity-based pass after the
class and template have been selected. Its roots are:

- external bindings referenced by the cleaned readable class;
- external bindings used by expressions that were actually rendered into the
  recovered template;
- simple identifiers in a canonical compiled `dependencies` array.

An exact ESM import specifier is retained when one of those roots resolves to
it. A local helper is copied only when its complete top-level dependency
closure is portable: function declarations, function or arrow initializers,
literal constants, and direct aliases. If that closure reaches a class,
eager call, or another unsupported initializer, the helper is omitted and the
artifact contains an explicit unresolved-symbol comment. This prevents one
module artifact from absorbing an arbitrary runtime graph.

When a compiled dependency root can be materialized, the emitter adds it to a
reconstructed `imports` list. This is an inspection-oriented equivalent of
the compiler dependency set, not a claim that the original decorator used the
same spelling or module organization.

When a dependency binding names another recovered component in the same source
module, the module emitter links it directly to the sibling's recovered class
name and renames sibling references consistently. Imports and portable helpers
shared by multiple components are emitted once.

For different source modules, Wakaru links only a local identifier connected to
an exported recovered component by an ordinary, unambiguous ESM import/export
edge. Low-level module results record the source/target component indices and
collision-free local name. The root artifact graph uses the final deduplicated
filenames to add relative imports such as
`import { ChildComponent } from "./child.angular"`. If that dependency binding
also appears in a template expression whose printed identifier cannot yet be
renamed safely, the edge stays unresolved instead of producing inconsistent
source.

Closure ModuleManager loader dependencies are deliberately not fabricated as
ESM edges. Bundling can also erase the package path that originally supplied a
component, directive, or pipe. Wakaru does not guess either form of provenance;
those relationships remain absent unless a generic module edge proves them.
This graph remains after module recovery, not in an unpacker or the Ivy
instruction classifier.

## Production feasibility validation

The committed primary corpus is a pinned Angular 22.0.8 CLI production
application under `crates/core/tests/bundles/angular-ivy-gen/`. It builds three
application components across a minified main chunk, shared Angular runtime
chunk, and lazy ESM chunk. The generator also passes those outputs through
Closure Compiler `SIMPLE`, and passes a separate retained-root producer entry
through Closure Compiler `ADVANCED`. A second minimally rooted `ADVANCED`
profile retains only the canonical component-definition role; all template
instruction roles remain Closure-renamed.

The original Angular, `SIMPLE`, and fully rooted `ADVANCED` producer forms
recover all three component definitions with non-empty inline templates.
Covered regions include element structure, static text, interpolation, event
listeners, property bindings, scoped styles, and cross-chunk runtime evidence.
The fixture also recovers a nested modern
`@if` / `@else` block, a selector-bearing projection slot, a local template
reference, and a pipe binding from both the Angular chunks and their Closure
`SIMPLE` and rooted `ADVANCED` aggregates. The isolated direct-compiler fixture
additionally recovers a modern `@for` / `@empty` repeater, its track expression,
loop-local reference, and captured-view listener as a complete artifact.
It also recovers a complete `@defer (on idle)` block with primary, loading,
placeholder, and error views. A generated legacy fixture recovers complete
`<ng-template [ngIf]>` and `<ng-template [ngForOf]>` forms while proving that
the authored `*ngIf` / `*ngFor` shorthand is absent. Separate generated
`prefetch on idle` and `hydrate on idle` components remain partial and prove
that those helpers are not mislabeled as an ordinary `on idle` trigger.
An assignment-backed derivative of that generated artifact proves the same
nested views after their function declarations are mechanically lowered to
stable predeclared assignments. Reassigning one of those bindings is a negative
fixture and remains partial.
The minimally rooted `ADVANCED` profile proves interpolation, chained
embedded-template, conditional, property, optimized repeater, defer, and idle
trigger role inference from runtime behavior. It also proves parent-context
traversal, the paired restore/reset view-state helpers, selector matrices,
attribute/class/style bindings, `<ng-container>`, bounded i18n, pure literal
bindings, `@let`, and HTML/SVG/MathML namespace transitions without retaining
their public role names. One generated conditional component exercises
Closure's inlined current-view capture, a restored listener, and a nested
property binding as a complete artifact. A separate generated component
recovers a complete deferred primary/placeholder pair, and the structural
component recovers `@for` / `@empty`. Unproven pipe and projection operations
remain explicit partial regions. Two additional minimally rooted components
retain Closure-renamed prefetch-idle and hydrate-idle helpers as negative
fixtures; neither is classified as the ordinary idle trigger.

A supplementary private production corpus is reported only in aggregate. The
current pass emitted 691 of 700 component candidates: 81 complete, 610 partial,
and 9 rejected. It rendered 52,669 of 60,812 observed runtime calls, with 6,463
unsupported and 1,680 malformed calls kept explicit. Repeater inference exposed
394 `@for` blocks across 66 of 132 module artifacts; all 132 artifacts parsed
as TypeScript, 12 loop collections remained unknown, and no track expression
was guessed. Compared with the preceding milestone, rendered-call coverage
rose from 44,096/51,402 (85.79%) to 52,669/60,812 (86.61%). The larger absolute
issue counts reflect roughly 9,400 newly traversed calls in child views that
were previously unreachable. This corpus contained no proven defer block, so
defer correctness remains established by the generated fixtures rather than
claimed from private data.

A second, smaller four-script local production corpus now emits all 120
component candidates: 63 complete, 57 partial, and none rejected. It renders
7,861 of 7,981 observed runtime calls, with 96 unsupported and 25 malformed
calls reported, and all four module-oriented artifacts parse as TypeScript.
The pre-hardening baseline emitted 117 of 120 candidates, only 17 complete,
with 1,038 unsupported and 223 malformed calls. Runtime-call denominators are
not treated as a strict coverage comparison across those milestones because
newly reachable child views and corrected failed-view accounting changed what
the analyzer observes.

Remaining partial regions in that smaller corpus are concentrated in
structural i18n, multi-statement restored listeners and unresolved view aliases,
ambiguous projection fallback/selector metadata, newer signal-form runtime
hooks without an authored-template equivalent, and ordinary application
declarations or calls. None of those operations is consumed based on a
corpus-specific minified callee name. They remain typed diagnostics until a
generic runtime contract and generated producer fixture justify recovery.

Closure output is requested with UTF-8 encoding because Angular's generated
field names contain Unicode identifiers. A non-UTF-8 compiler output profile
can replace those identifier characters and produce text that is not valid
JavaScript.

The committed `ADVANCED` producer explicitly exports the component classes,
their compiled definition values, and a narrow canonical Ivy runtime map
through externed global properties. Exporting classes alone is insufficient:
Closure can still remove unobserved static definition assignments. Generic
unrooted `ADVANCED` remains a negative experiment because the component
metadata no longer exists for a decompiler to recover.

`ADVANCED` may irreversibly rename ordinary component fields. Wakaru preserves
the remaining binding consistently (for example, between a class field and an
`@if` expression) but does not invent the original property name. Descriptor
fields are different: when structural evidence remains unambiguous, such as a
projection-selector string array used by a template with projection
instructions, their semantic role can still be recovered.

Local `WHITESPACE_ONLY` experiments and complete public application bundles
remain supplementary stress tests. They do not define the committed Angular
vocabulary or success baseline.

Legacy `*ngIf` / `*ngFor` spelling is not reconstructed from a bare embedded
view. That spelling is erased by compilation, and the same low-level template
shape can be driven by a custom structural directive. The generated Angular 22
fixture proves that ordinary template/property recovery keeps a complete,
readable `<ng-template>` form. Re-sugaring requires proven directive/input
metadata and is intentionally lower priority than the optimized control-flow
families observed in current production measurements.

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
6. projection, local references, and pipes.
7. artifact-local helpers, imports, and materializable dependencies.
8. rooted Closure `ADVANCED` validation and representative-corpus profiling.
9. repeaters, loop-local aliases, and captured-view listeners.
10. assignment-backed embedded-view functions and stable aliases.
11. minimally rooted Closure `ADVANCED` role inference from runtime behavior.
12. Closure view-state role families and optimizer-inlined view captures.
13. module-oriented artifacts and view-local unsupported-region placement.
14. cross-artifact component relationships from proven ESM symbol edges.
15. generated and Closure-renamed defer/repeater control-flow families.
16. selector/constant-table families, expression interpolation, pure literals,
    bounded i18n, `@let`, and HTML/SVG/MathML namespace transitions.

At each checkpoint verify that no unpacker contains Ivy roles, no Ivy module
branches on a bundle format, and no normal JavaScript rewrite depends on
artifact emission.
