# Google Tag Manager containers

A container fetched from `https://www.googletagmanager.com/gtm.js?id=GTM-…` is
one script with two parts: a `var data = {…}` object holding the site's tags,
variables, triggers and custom templates, and Google's Closure-compiled
runtime, which is the same in every container. The container data is the part
worth reading — marketing teams add Custom HTML tags and Custom JS variables
through GTM, often outside the normal code review process.

`--unpack` recognizes a container by that object shape — a `var data = {…}`
whose `resource` carries `macros`, `tags`, `predicates` and `rules` arrays —
read from the AST and not from the file name, so a container saved by hand
works too. Explicit file inputs and stdin are checked; directory scans are
not. The runtime is skipped: it is the same library in every file, and
running wakaru without `--unpack` still decompiles the whole input.

```
wakaru gtm.js --unpack=auto -o out/
```

The pieces are written under `-o`:

| Path | What it is |
|---|---|
| `macros/jsm-<n>.js` | the body of a Custom JS variable, `n` being its index in the container's macro array |
| `tags/html-<tag_id>-<n>.js` | one inline script from a Custom HTML tag, in the order it appears in the tag |
| `index.md` | every macro index with its `function` type and name, the tags with the triggers that fire or block them, and the URLs of external scripts |

With more than one input, a container's files go under a directory named after
its input file (`gtm/macros/…`), with `-2`, `-3`… added when two inputs share a
file name.

Each script is then decompiled on its own, as a single file. It does not go
through bundle detection: a script is not a bundle, and the heuristic
scope-hoist split would cut an ordinary IIFE apart. `--raw` writes each script
as extracted. A script the pipeline cannot parse is written as extracted and
reported as `decompile_failed`, which makes the run exit nonzero like any
failed module. In `--json` output the scripts are modules; `index.md` is not.
They have no provenance entries and no output source maps.

The Rust façade exposes the extraction as `wakaru::gtm::extract_container`: it
returns the scripts, macros, external URLs and unsupported template parts as
data, and the caller decompiles the scripts. File names and `index.md` are CLI
layout. The parsing lives in `crates/core/src/gtm.rs`; the CLI wiring is
`push_explicit_source` and `decompile_gtm_scripts` in `crates/cli/src/main.rs`.

## Macro references

A container stores editable text as template arrays — `["template", "literal",
<escape>, …]` — where an escape is `["escape", ["macro", n], mode, …]`, a
reference to the macro at index `n`. A field with no reference is stored as a
plain string instead. The generated code does not resolve references, because
a macro can be a constant with no name and a resolved name would change every
time the container is edited. The text keeps the identifier `__gtm_macro_<n>`
and `index.md` says what `n` is.

The modes say where the reference sits. These were read from public
containers (wild-observed):

| Modes | Position | Written as |
|---|---|---|
| `8, 16` | code | `__gtm_macro_<n>`, a reference |
| `7` | inside a string literal | the literal closed and the reference concatenated with the same quote, so the data flow stays visible; the pipeline may fold it into a template literal |
| `9` | inside a regular expression literal | `__gtm_macro_<n>` as text of the pattern |
| `3` | an HTML attribute value | `__gtm_macro_<n>` as text |
| `12` | a URL component, such as a query parameter | `__gtm_macro_<n>` as text |
| anything else | unknown | the identifier `__gtm_macro_<n>_escape_<modes>` and a `gtm_unsupported_escape` warning |

An unknown combination keeps an identifier rather than a comment: a comment
would not survive the pipeline, and dropping the reference would delete an
argument or operand.

For mode 7 the quote character comes from a scan of the current script that
tracks strings, comments and regular expression literals. A `/` counts as the
start of a regular expression after punctuation that cannot end an operand; a
regular expression right after a keyword such as `return` is read as division
and can still mislead the scan.

## Tags and triggers

`index.md` lists every tag the container carries — its `tag_id` and GTM's type for
it (`__html` is Custom HTML, `__gaawe` a GA4 event) — and the triggers that fire
or block it, by their row in the trigger table. A trigger's condition is read out
of the container's compiled `rules` array: the `if` predicates must all hold and
no `unless` predicate may, and each predicate is written in words (`_eq` equals,
`_cn` contains, `_sw` starts with, `_re` matches, `_css` matches a CSS selector).
An operand is the macro's `vtp_name`, or its type and `vtp_component` when it has
no name (`__e`, `__u:PATH`). When a macro with a different definition reads the
same, the operand also carries its row in the macro table (`__u:QUERY#4`): two
query-string variables differ only in their key, and two Custom JS variables only
in their body. A container often repeats a definition, such as a second `__e`;
the copies read the same value, so a copy reads as its first row and keeps the
bare label when nothing else shares it. The
runtime expands an argument by its first element, as it does a tag field, so a
list is `["list", …]` and a value with macros in it is `["template", …]`. An
`any_of` comparison's list holds alternatives and reads as `or`; any other list
reads as a comma list, and a template reads as one quoted value with each macro as
`{{…}}`. The runtime honors `ignore_case` only for `_re`, so only a `_re`
predicate reads `(ignoring case)`. An operator outside that table keeps its GTM
name rather than a guessed word.

The runtime stores a rule's `add` and `block` instructions as positions in the tag
array; the extraction turns those into the `tag_id`s a reader sees. A trigger can
fire no tag and only block, and the other way around.

## Custom HTML tags

GTM rewrites most script elements it serves: an inline script gets
`type="text/gtmscript"`, and an external one moves its `src` into
`data-gtmsrc`. Tags also keep plain `<script>` and `<script src>` elements, so
both forms are taken. A script is JavaScript when it has no `type`,
`text/gtmscript`, or a JavaScript MIME type or `module`; other types (JSON
data, templates) and other elements, such as `<noscript>` fallbacks, are
skipped. An external script has no body, so its URL is listed in `index.md`.

A tag can hold several scripts, which is why they are numbered rather than
concatenated; the number is the position among the tag's scripts, so a tag
whose first script is external starts at `-1` (its only file may be
`html-41-1.js`).

Custom template opcode decompilation is not implemented: a custom template
stored as compiled opcodes is left alone.
