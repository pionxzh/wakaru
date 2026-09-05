# Docs site (`docs-site/`)

Status: scaffolded, all planned pages written (17); not yet deployed.

The user-facing documentation site, served at `wakarujs.com/docs`. This
records the decisions, the sidebar plan, and what remains before launch.

## Decisions

- **Framework: Fumadocs** (Next.js). Chosen over Starlight after comparing:
  SPA client-side navigation and a stronger default UI won; the playground is
  already React, so the stack is familiar. VitePress (theming depth) and
  Docusaurus (React runtime everywhere, dated look) were rejected earlier.
  De-risking: content stays vanilla MDX + frontmatter so it ports anywhere;
  pin Next/fumadocs and upgrade on our own schedule; Orama local search (no
  external service).
- **Audience split**: `docs-site/` is user docs (how to use). Repo `docs/` is
  engineering docs (how it works, why) and does not shrink because the doc
  site exists. Same topic, different write-ups.
- **Deployment**: separate Vercel project (root directory `docs-site/`),
  proxied from the main site like the playground. Landing stays static
  no-build.

## Architecture notes (the gotchas)

- `basePath: '/docs'` in `next.config.mjs`; docs pages live at the **app
  root** (no `/docs/docs`). The scaffold's `(home)` group was removed.
- Search: `fetch()` does not apply basePath, so the RootProvider sets
  `search.options.api = '/docs/api/search'` explicitly.
- `proxy.ts` markdown-negotiation rewrites must prefix `nextUrl.basePath`
  back onto rewrite targets (`nextUrl.pathname` has it stripped), and must
  skip internal prefixes (`/api/`, `/og/`, `/llms.`, `/llms-`, `/_next/`)
  now that docs pages occupy the root.
- Markdown negotiation works: any page serves markdown via `Accept:
  text/markdown` or a `.md` suffix; `llms.txt` / `llms-full.txt` are wired.

## Sidebar structure

Sections via root `meta.json` separators; folders `guides/`, `reference/`,
`project/` own their ordering. Only pages that exist appear — no
"coming soon" stubs.

| Section | Page | Status | Source material |
| --- | --- | --- | --- |
| Introduction | Quick Start (`index`) | **Phase 1, written** | README |
| Introduction | What is Wakaru | **Phase 1, written** | landing FAQ + README |
| Guides | Unpack a bundle | **Phase 1, written** | `docs/cli.md` |
| Guides | Unminify a file | **written** | `docs/cli.md` |
| Guides | Obfuscated code | **written** | README "Works with other tools" |
| Guides | Rewrite levels | **Phase 1, written** | `docs/rewrite-assumptions.md` |
| Guides | Source maps | **written** | `docs/cli.md` |
| Guides | Bun single-file executable | **written** | `docs/bun-standalone.md`, `docs/cli.md` |
| Guides | Vue SFC | **written** | `docs/vue-decompile.md`, `docs/cli.md` |
| Guides | Playground | **written** | `playground/src` |
| Guides | Coding agents | **written** | `skills/wakaru/SKILL.md` |
| Reference | CLI | **Phase 1, written** | `docs/cli.md` |
| Reference | Supported inputs | **written** | README + `docs/cli.md` |
| Reference | Output & warnings | **written** | `docs/cli.md`, `skills/wakaru/SKILL.md` |
| Project | FAQ | **Phase 1, written** | landing FAQ |
| Project | Troubleshooting | **written** | `docs/cli.md`, `docs/unpacking.md`, CLAUDE.md large-inputs note |
| Project | Correctness | **written** (may split later) | `docs/test262-roundtrip.md`, README |

Added after the first 14 (gap review, 2026-09-05): Playground, Obfuscated
code, Troubleshooting. An Installation page was drafted and folded into
Quick Start as an "Install" section: one command plus a Releases link did
not justify a page. Considered and deferred: a JSON output
field reference (currently split across CLI and Output & warnings), a
one-paragraph Rust crate page pointing at docs.rs, a Contributing page, and
Use cases (README has five; could fold into What is Wakaru). Not planned:
Changelog (link GitHub Releases), package inventory (in development), a
Formatter page (one flag).

Naming conventions: short phrase titles ("Unpack a bundle", "Rewrite
levels"); "Bun single-file executable" singular (matches Bun's own docs); no
Configuration section (three levels + a few flags don't justify one).

## Source maps page: playground mapping embed

The Source maps guide should show the playground's rainbow line-mapping
view. Options, in order:

1. **Launch version**: screenshot + a prepared playground share URL with the
   mapping toggle on. Zero engineering.
2. **iframe embed** (the preferred direction): add an `?embed=1` mode to
   the playground that hides the header/controls, then iframe it. Small
   playground change.
3. **Static widget**: precomputed mapping data + CSS highlighting, no
   Monaco/wasm. Only if the page earns it.

## Richness backlog (pages read fine, could show more)

Review verdict on the first drafts: prose is clean but sometimes boring.
Fix with examples, tables, and screenshots, not with more words.
Done so far: the before/after example on What is Wakaru. Still open:

- Unpack a bundle: output tree example done (four-module webpack bundle,
  numeric IDs).
- Source maps: mapping screenshot done (launch version). The `?embed=1`
  iframe stays as the next step.
- Correctness: a per-matrix numbers table once the page grows; possibly
  split into one page per harness later (agreed low priority).
- Screenshots live in `docs-site/public/screenshots/` (retina PNG, header
  and toolbar cropped to what the feature needs, version/hash removed so they
  do not go stale). Four exist: playground, roundtrip-diff, mapping, vue-sfc.
  Add more only where they carry information.

## Sync obligations

The Correctness page cites the Test262 and repro stats (62,061 / 97.4%).
Those figures are the same ones `scripts/repro/collect-stats.mjs --check`
enforces for README and the landing page. The check does not cover the
docs site yet, so a stats regeneration must update
`docs-site/content/docs/project/correctness.mdx` by hand (or the check
script should learn the new surface).


`reference/cli.mdx` joins the existing `docs/cli.md` ↔
`skills/wakaru/SKILL.md` sync pair. `docs/cli.md` stays the source of truth
for behavior detail; a CLI flag or output change now updates three surfaces
in the same commit. On merge, update the sync rule in `CLAUDE.md` /
`docs/README.md` ("CLI flag or output changes" row) to name the docs-site
page.

## Launch checklist

Deployment follows the playground pattern: a GitHub Actions workflow
(`.github/workflows/docs-site.yml`) runs `vercel build` + `vercel deploy
--prebuilt --prod` from `docs-site/` on pushes to `main` that touch
`docs-site/**`. No root-directory setting is involved; the CLI uploads the
directory it runs in. Secrets: `VERCEL_TOKEN`, `VERCEL_ORG_ID` (shared with
the playground) and `VERCEL_DOCS_PROJECT_ID`.

1. Vercel project `wakaru-docs` created (2026-09-05, CLI). Production URL:
   `https://wakaru-docs.vercel.app`.
2. Add the `VERCEL_DOCS_PROJECT_ID` repo secret and the deploy workflow.
3. First production deploy, then confirm `https://wakaru-docs.vercel.app/docs`
   serves the site.
4. Proxy routes in `website/vercel.json` (`/docs`, `/docs/(.*)`): done.
5. "Docs" link in the landing nav and footer.
6. `website/robots.txt` lists `https://wakarujs.com/docs/sitemap.xml`; the
   docs app generates it from `app/sitemap.ts`. The hand-written
   `website/sitemap.xml` keeps covering only `/` and `/playground/`.
7. `AGENTS.md` / `docs/README.md` CLI sync rule names the docs page: done.
8. Push and merge; the workflow deploys on the first push touching
   `docs-site/`. Until then the main-site proxy would point at an empty
   project, so merge the proxy routes together with, not before, the first
   deploy.
