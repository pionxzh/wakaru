# Docs Site (`docs-site/`)

The user-facing documentation at `wakarujs.com/docs`. This document covers
how the site is built and deployed, what to keep in sync when the product
changes, and the conventions its pages follow. For the engineering docs
under `docs/`, see [README.md](README.md).

## Audience split

`docs-site/` is user documentation: how to use Wakaru, which flag for which
situation, how to read the output. Repo `docs/` is engineering
documentation: how it works, why it is designed this way, which edges not to
cross. The two do not share content. A topic that appears in both gets two
different write-ups, and `docs/` does not shrink because the site exists.

## Stack

Next.js + [Fumadocs](https://fumadocs.dev), chosen over Starlight for
client-side navigation and a stronger default UI (the playground is already
React). Content is vanilla MDX with frontmatter so it ports to any docs
framework. Search is Orama, indexed at build time, no external service.
Next and fumadocs versions are pinned and upgraded on our own schedule.

The app also serves per-page OG images, `llms.txt` / `llms-full.txt`, and
markdown for any page via `Accept: text/markdown` or a `.md` suffix.

## basePath gotchas

The site is proxied under `/docs`, so `next.config.mjs` sets
`basePath: '/docs'`. Things that follow from that:

- Docs pages live at the **app root** of `docs-site/app`. Putting them under
  an inner `docs/` route would serve them at `/docs/docs/...`.
- `fetch()` does not apply the basePath. The `RootProvider` sets
  `search.options.api = '/docs/api/search'` explicitly.
- `proxy.ts` markdown-negotiation rewrites must add `nextUrl.basePath` back
  onto rewrite targets (`nextUrl.pathname` has it stripped), and must skip
  internal prefixes (`/api/`, `/og/`, `/llms.`, `/llms-`, `/_next/`) because
  docs pages occupy the root.
- `app/sitemap.ts` emits absolute `https://wakarujs.com/docs/...` URLs.
  `website/robots.txt` lists it next to the landing sitemap.
- `docs-site/vercel.json` declares `"framework": "nextjs"`. The Vercel
  project was created from the CLI and has no framework preset; without the
  declaration `vercel build` uses the static builder and every route 404s.
  The same file redirects the bare deployment root to `/docs`.

## Deployment

Same pattern as the playground. `.github/workflows/docs-site.yml` runs
`vercel build` + `vercel deploy --prebuilt --prod` from `docs-site/` on pushes
to `main` that touch `docs-site/**`. Secrets: `VERCEL_TOKEN` and
`VERCEL_ORG_ID` (shared with the playground) plus `VERCEL_DOCS_PROJECT_ID`.
The Vercel project is `wakaru-docs`; `website/vercel.json` proxies `/docs`
and `/docs/(.*)` to it. The landing page stays static and no-build.

Local preview: `cd docs-site && npm run dev`, then open
`localhost:3000/docs` (the bare root is not served).

## Sync obligations

- **CLI behavior.** `docs/cli.md` is the source of truth. A flag or output
  change updates `docs/cli.md`, `skills/wakaru/SKILL.md`, and
  `docs-site/content/docs/reference/cli.mdx` in the same commit. The
  AGENTS.md task table names all three.
- **Stats figures.** The Correctness page cites the repro aggregate
  (`97.4% across 1,858 test shapes`) and the Test262 totals.
  `scripts/repro/collect-stats.mjs --check` verifies the repro citation on
  README, the landing page, and the Correctness page; regenerating stats
  without moving the copy fails the check. The Test262 figures are not
  checked by a script: when `scripts/correctness/test262-stats.json`
  changes, update the Correctness page by hand.
- **Playground features.** The Playground guide describes the controls and
  modes. A playground UI change that adds or renames a control updates that
  page.

## Content conventions

Sidebar sections come from separators in the root `meta.json`; the
`guides/`, `reference/`, and `project/` folders own their ordering. Only
pages that exist appear, no "coming soon" stubs. Titles are short phrases
("Unpack a bundle", "Rewrite levels"); "Bun single-file executable" is
singular to match Bun's own docs. There is no Configuration section: three
levels and a few flags do not justify one.

Prose follows the plain-English register (short sentences, no em dashes, no
semicolons, claims literally true). Support lists are flat bullets, one
scan-target per line. Engine vocabulary ("pipeline", "structural detection")
stays out of guides. Binary format names appear only on the Bun page and the
CLI reference. Internal tooling is labeled as such. Screenshots live in
`docs-site/public/screenshots/` as retina PNGs, cropped to the feature they
show, with version and commit hash removed so they do not go stale.

Source material per page, for whoever edits them later:

| Page | Source material |
| --- | --- |
| Quick Start (`index`) | README |
| What is Wakaru | landing FAQ, README |
| Unpack a bundle | `docs/cli.md`, `docs/unpacking.md` |
| Unminify a file | `docs/cli.md` |
| Obfuscated code | README "Works with other tools" |
| Rewrite levels | `docs/rewrite-assumptions.md` |
| Source maps | `docs/cli.md` |
| Bun single-file executable | `docs/bun-standalone.md`, `docs/cli.md` |
| Vue SFC | `docs/vue-decompile.md`, `docs/cli.md` |
| Playground | `playground/src` |
| Coding agents | `skills/wakaru/SKILL.md` |
| CLI | `docs/cli.md` |
| Supported inputs | README, `docs/cli.md` |
| Output & warnings | `docs/cli.md`, `skills/wakaru/SKILL.md` |
| FAQ | landing FAQ |
| Troubleshooting | `docs/cli.md`, `docs/unpacking.md`, the large-inputs note in AGENTS.md |
| Correctness | `docs/test262-roundtrip.md`, README |

## Backlog

- Correctness: a per-matrix numbers table, possibly one page per harness
  later (low priority).
- Pages considered and deferred: a JSON output field reference (currently
  split across CLI and Output & warnings), a one-paragraph Rust crate page
  pointing at docs.rs, Contributing, Use cases. Not planned: Changelog (link
  GitHub Releases), package inventory (in development), a Formatter page.
- Source maps page: the playground embed (`?embed=1`) is in place. A static
  widget with precomputed mapping data would remove the Monaco and WASM
  download from that page; only worth it if the page earns traffic.
