# Debug and experimental CLI (`wakaru debug`)

`wakaru debug` is where Wakaru exposes development tools and early or
experimental features before their best long-term interface is clear. Anyone
can use these commands, but they favor exploration and feedback over
compatibility. The command group is hidden from top-level `wakaru --help`; run
`wakaru debug --help` to see what is available in your installed version.

## Compatibility policy

The commands and behavior described here document the current version, not a
stability promise:

- A minor release may add, remove, or rename a `debug` command or flag.
- Argument signatures, human-readable output, JSON schemas, findings, and exit
  behavior may change in a minor release without a deprecation period.
- Hidden or experimental status does not relax Wakaru's correctness, path
  safety, or fail-closed requirements.

If you automate a `debug` command outside the Wakaru repository, pin the Wakaru
version and review this document when upgrading.

If one of these commands is useful in your work, we want to hear how you use
it. Real workflows, representative inputs, missing capabilities, awkward
parts, desired output, and ideas from experience can all help shape a better
supported feature.

The compatibility policy above applies to the CLI command group only. The
published Rust `wakaru::debug` namespace is governed by its rustdoc and
[public-api.md](public-api.md).

## `debug trace`

```bash
wakaru debug trace input.js
wakaru debug trace input.js --from RemoveVoid --until UnEsm
wakaru debug trace input.js --all
wakaru debug trace input.js -o trace.txt --force
```

Runs the normal single-file rule pipeline and prints the initial source plus a
unified diff for each rule that changes the rendered output. `--all` includes
unchanged rules; `--from` and `--until` select an inclusive rule range;
`--level` selects the rewrite level; `-m` / `--source-map` supplies a source
map. Output goes to stdout unless `-o` is supplied. A `--from` rule that the
pipeline runs after the `--until` rule is rejected as an option error. When
no rule in the traced range changed the rendered output, the command prints a
single note line saying so instead of an empty trace.

This command is intentionally single-file only. Full bundle recovery uses the
two-phase cross-module pipeline, so tracing a bundle as one source file would
misrepresent that execution path. See [debugging.md](debugging.md) for the
rule-regression workflow.

## `debug normalize`

```bash
wakaru debug normalize input.js
wakaru debug normalize input.js --rename --format
echo 'function f(a){return g(a)}' | wakaru debug normalize --rename
```

Parses and reprints one source file for structure-oriented comparisons.
`--rename` applies scope-correct deterministic alpha-renaming while preserving
free/global names, and `--format` runs the final formatter. The reproduction
matrices use this command to compare differently mangled forms.

The input is optional when stdin is piped; use `-` explicitly to select stdin.
Output is written to stdout.

For example, `--rename` turns:

```js
function load(appId) {
    return fetchModule(appId);
}
```

into:

```js
function $0($1) {
    return fetchModule($1);
}
```

The local function and parameter receive deterministic names, while the free
reference `fetchModule` remains unchanged.

## `debug enumerate-chunks`

```bash
wakaru debug enumerate-chunks bundle.js
cat bundle.js | wakaru debug enumerate-chunks -
```

Inspects one JavaScript input for statically enumerable lazy-chunk references
and writes JSON to stdout. It parses and detects the input, but does not unpack
or decompile modules, write files, or make network requests. The input is
optional when stdin is piped; use `-` explicitly to select stdin.

The current root shape is:

```json
{
  "input": "bundle.js",
  "detected_format": "webpack5",
  "enumeration": {
    "public_path": {"status": "static", "value": "/assets/"},
    "assets": [{
      "kind": "js",
      "status": "enumerated",
      "template": "[id].[map].js",
      "urls": [{"chunk_id": "101", "url": "101.aaaa1111.js", "source": "filename_map"}]
    }],
    "relative_imports": [
      {"specifier": "./lazy-beta.js", "kind": "dynamic_import"}
    ]
  }
}
```

`detected_format` is `null` when structural bundle detection finds no known
format. `enumeration` is `null` when neither of these fail-closed sources
produces a fact:

- `assets` describes webpack 4/5 runtime filename tables
  (`__webpack_require__.u`, `.miniCssF`, or webpack 4's `jsonpScriptSrc`).
- `relative_imports` describes literal relative specifiers from `import`,
  `export ... from`, and `import()`. The `kind` is `import`, `export_from`, or
  `dynamic_import`. Bare packages, absolute URLs, and computed specifiers are
  excluded. The field is omitted when empty.

For `assets`, `kind` is `js` or `css`. `status` is `enumerated`,
`no_static_chunk_ids`, or `dynamic_template`. `urls[].source` is
`filename_map` when an id comes from the filename lookup table, or
`ensure_call` when it comes from a binding-correlated literal webpack load
site. `template` is a debug rendering using placeholders such as `[id]`,
`[map]`, and `[map|id]`; it is absent for dynamic templates and is not a
machine-parseable contract.

`public_path.status` is `static`, `script_relative`, `runtime_computed`, or
`not_found`. A `value` is present only for a proven literal static path or the
literal suffix of webpack's canonical script-relative path. Asset URLs remain
relative and never include the public path. A caller can resolve
`public_path.value + urls[].url` against the bundle URL only for `static` and
`script_relative`; for the other statuses the complete request URL is not
known. Each `relative_imports[].specifier` is already a relative sibling URL and
should instead be resolved directly against the entry URL.

Runtime filename tables from other bundler families are not interpreted.
Their literal relative ESM specifiers can still appear in `relative_imports`.
Fetching any reported URL is the caller's responsibility.

## `debug validate`

```bash
wakaru debug validate out/
wakaru debug validate out/ --json
wakaru debug validate out/ --input bundle.js
```

Validates a normal unpack output directory as one emitted-module graph. It
reports dangling relative references, imports or re-exports of missing or
star-ambiguous names, local export clauses that name no declared binding,
duplicate exports or conflicting declarations (including nested block, switch,
loop, function-parameter/body, and catch-parameter scopes), and writes to
imported or `const` bindings. It also reports unresolved `module` / `exports`
runtime uses left in ESM output. Direct safe `typeof module` / `typeof exports`
probes are excluded.

`.mjs` / `.mts` files and in-tree static or dynamic import targets use the
module source goal even when they contain no import/export declaration.
Explicit `.cjs` / `.cts` files retain the script/CommonJS source goal even when
imported by ESM.

Const/import-write findings use resolved binding identity, but do not analyze
reachability, logical-assignment short circuits, or caught exceptions. They
can therefore occur in code that loads and executes successfully. A finding
also does not establish that Wakaru introduced the write: `--input` compares
free identifiers only and does not suppress pre-existing const/import writes.

Free identifiers are reported as `unresolved_reference` only when the graph
proves them wrong. Without `--input`, that proof is structural: exactly one
other emitted module declares the same name at module scope. Names declared by
several modules are ambiguous reused locals and stay silent. With repeatable
`--input PATH` file or directory arguments, the original bundle is parsed too,
and any free identifier in the output that is free nowhere in the input is
reported. Input evidence is authoritative: a name used freely by the input is
never reported even when the structural proof would match it. If any input
fails to parse, input comparison is skipped rather than run on a partial
baseline, and the failed input receives a `parse_error` finding.

ECMAScript built-ins and module-runtime names are never reported. For the
structural proof, well-known host globals declared by shim modules are excluded
too. Other writes to undeclared identifiers remain environment-dependent host
global accesses unless input or sibling-declaration evidence proves the name
wrong.

Human-readable findings use:

```text
filename:line:column: kind: message
```

The current JSON shape is:

```json
{
  "modules": 1,
  "findings": [{
    "filename": "entry.js",
    "line": 12,
    "column": 7,
    "kind": "assign_to_import",
    "message": "assignment to imported binding \"value\""
  }]
}
```

Locations are one-based. The command currently exits nonzero when findings
exist and errors when the directory contains no accepted JavaScript files.
The recursive scan accepts `.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx`,
`.mts`, `.cts`, and extensionless emitted modules, including modules below
`node_modules`; hidden paths and unrelated extensions are excluded.

Validate normal output only. Raw unpack output has no usable module-graph
contract. Directory validation reads only the emitted files, so it also scans
artifacts retained after a failed factory recovery. Unresolved numeric webpack
runtime calls in those artifacts are not interpreted as relative module edges.
See [debugging.md](debugging.md) for the fixture and regression workflow.
