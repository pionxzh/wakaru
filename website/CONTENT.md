# Landing Page Content Brief

Working document for the website rebuild. This is the *content* spec — copy,
structure, claims, and target queries. Visual design is out of scope here.

## Positioning

One sentence, used everywhere consistently:

> Wakaru turns production JavaScript — bundled, transpiled, minified — back
> into readable modules.

Differentiator line (the thing no competitor can say):

> The JS decompiler with receipts: the only one backed by a
> semantic-equivalence test suite.

Category words to use, because they are what people search: *JavaScript
decompiler*, *unminify*, *unbundle*, *unpack a bundle*. Avoid inventing new
category names.

Tone rules:

- Every claim carries a number or a link to the harness that produces it.
  No adjectives doing load-bearing work ("blazing", "powerful", "SOTA").
- Be honest about limits in the main copy, not in a footnote: no
  deobfuscation (point to webcrack), mangled names stay mangled without a
  source map (point to LLM renamers / `--source-map`).
- Show real, unedited tool output. Never hand-polish a demo.

## Page map

1. **Landing** (`/`) — hero, live demo, receipts, comparison, use cases, FAQ.
2. **Playground** (`/playground`) — exists; the landing page's primary CTA.
3. Optional later: one page per use case (see SEO section) — thin pages are
   worse than none, so only add when there is real content per page.

## Landing page sections, in order

### 1. Hero

- H1: **Unpack. Unminify. Understand.**
- Subhead: the positioning sentence.
- Primary CTA: "Open the playground". Secondary: the `npx @wakaru/cli` one-liner
  with a copy button.
- Directly below or beside: the before/after sample (same one as the README —
  minified Babel async output in, clean `async`/`await` + `??` + `?.` ESM
  out). If feasible, make it the playground embedded with the sample
  preloaded: the demo *is* the product.

### 2. What it handles

Three short columns matching the three transformations users need reversed:

- **Bundlers** — webpack 4/5 (chunks, multi-file), esbuild, Bun, Browserify,
  SystemJS, AMD/UMD, scope-hoisted ESM (Rollup/Vite).
- **Transpilers** — Babel / TypeScript / SWC helper recovery: async/await,
  classes, spread, JSX, enums, optional chaining, and more.
- **Minifiers** — Terser/SWC/esbuild artifact reversal, alias inlining,
  IIFE flattening.

Plus one line on rewrite levels: `minimal` for auditing, `standard` default,
`aggressive` for maximum readability.

### 3. Receipts ("Tested like a compiler")

Three stat cards, each linking to its source:

| Stat | Copy | Source link |
|---|---|---|
| **0** | semantic failures across 41,000+ runnable Test262 round-trip cases (3 producers × 20 slices + module graphs) | `docs/test262-roundtrip.md` |
| **97.7%** | pattern recovery across 1,545 transpiler × minifier shapes | `scripts/repro/stats.json` |
| **seconds** | to unpack + decompile a 10 MB production bundle (4,500+ modules); Rust + SWC, parallel | GitHub Releases / bench notes |

One sentence under the cards explaining the round-trip methodology (original,
transformed, and decompiled code all pass the same Test262 test) — the
methodology is the credibility, not the number alone.

### 4. Honest comparison

The README comparison table (wakaru / webcrack / humanify), plus the
composition pitch: *these tools compose* — webcrack strips obfuscation,
wakaru recovers structure, humanify names things. Show the two-command
pipeline. This section targets comparison searches and builds trust by
conceding webcrack's strength explicitly.

### 5. Use cases

One short block each, headed by the searcher's own words (these are the H2s
for SEO):

- Security review & bug bounty ("read what a site actually ships")
- Incident response & malware triage (`minimal` level: the semantics you
  review are the semantics that ran)
- Recover lost source ("all that's left is `dist/`"; `wakaru extract` for
  shipped source maps)
- Debug a third-party SDK
- Supply-chain inspection

### 6. FAQ

Written as literal search queries; use FAQ schema markup:

- *How do I unminify JavaScript?*
- *How do I decompile a webpack bundle?* / *extract modules from a bundle?*
- *Can Wakaru deobfuscate JavaScript?* (No — webcrack, then wakaru; show the
  pipeline.)
- *Can it recover original variable names?* (Source maps yes; otherwise
  structure only — pair with an LLM renamer.)
- *Is the output guaranteed to behave the same?* (Levels + the Test262
  harness; `minimal` is the auditing mode.)
- *Does it run in the browser?* (Yes — the playground is the WASM build; no
  code leaves the machine.)
- *Is it free?* (Apache-2.0, open source.)

### 7. Footer

npm, GitHub, Releases, Telegram, playground, docs index.

## SEO targets

Primary queries the copy must contain naturally (headings > body):

- unminify javascript / unminify js online
- decompile javascript / javascript decompiler
- decompile webpack bundle / unpack webpack / webpack unbundle
- extract modules from a js bundle
- recover source code from minified javascript
- reverse engineer javascript bundle
- webcrack alternative (comparison section covers this honestly)

Mechanics (cheap, high-leverage):

- The playground page should be indexable with its own title/description
  ("Unminify and unpack JavaScript online — free, in-browser, no upload").
- GitHub repo topics: `javascript-decompiler`, `unminify`, `unbundle`,
  `reverse-engineering`, `webpack`, `esbuild`, `deobfuscation` (traffic term;
  the README explains the webcrack pairing).
- One architecture blog post ("How Wakaru turns a bundle back into modules")
  — pipeline diagram + the fact system; engineers share how-it-works posts,
  and backlinks matter more than keywords.

## Claims inventory

Every public claim and where it comes from. If a claim's source moves, update
the copy in the same change (see the Definition of Done in AGENTS.md).

| Claim | Source |
|---|---|
| 0 failures / 41,000+ runnable Test262 cases | `scripts/correctness/test262-stats.json`, `docs/test262-baselines/` |
| 97.7% of 1,545 shapes recovered | `scripts/repro/stats.json` |
| 10 MB bundle, 4,500+ modules, seconds | keep phrasing generic; re-verify before changing numbers |
| Bundle format list | `docs/architecture.md` unpacker section |
| ~60 restoration rules | `crates/core/src/rules/pipeline.rs` |
| Rewrite level semantics | `docs/rewrite-assumptions.md` |

Do not add new numbers to the site without adding them here with a source.
