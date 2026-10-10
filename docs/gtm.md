# Google Tag Manager containers

A container fetched from `https://www.googletagmanager.com/gtm.js?id=GTM-…` is
one script with two parts: a `var data = {…}` object holding the site's tags,
variables, triggers and custom templates, and Google's Closure-compiled
runtime, which is the same in every container. The container data is the part
worth reading — marketing teams add Custom HTML tags and Custom JS variables
through GTM, often outside the normal code review process.

`--unpack` recognizes a container by that object shape, read from the AST and
not from the file name, so a container saved by hand works too. The runtime is
skipped: it is the same library in every file, and running wakaru without
`--unpack` still decompiles the whole input.

```
wakaru gtm.js --unpack=auto -o out/
```

The pieces are written under `-o`:

| Path | What it is |
|---|---|
| `macros/jsm-<n>.js` | the body of a Custom JS variable, `n` being its index in the container's macro array |
| `tags/html-<tag_id>-<n>.js` | one script from a Custom HTML tag, in the order it appears in the tag |
| `index.md` | every macro index with its `function` type and name, and the URLs of external scripts |

Each JavaScript piece then goes through the normal pipeline, so the emitted
code is decompiled like any other file.

## Macro references

A container stores editable text as template arrays — `["template", "literal",
<escape>, …]` — where an escape is `["escape", ["macro", n], mode]`, a
reference to the macro at index `n`. The generated code does not resolve those
references, because a macro can be a constant with no name and a resolved name
would change every time the container is edited. The text keeps the identifier
`__gtm_macro_<n>` and `index.md` says what `n` is.

The mode says where the reference sits:

- **8 and 16** — code position, so `__gtm_macro_<n>` drops in as it is.
- **7** — inside a string literal. A bare identifier there would become plain
  text and a reader (or a SAST tool) would lose the data flow, so the literal
  is closed and the reference concatenated with the same quote character. The
  pipeline may then fold it into a template literal.
- **anything else** — kept as a comment placeholder, and reported as a warning.
  Mode `12` occurs in real containers and its meaning is not established yet.

## Custom HTML tags

GTM rewrites the script tags it manages: an inline script carries
`type="text/gtmscript"`, and an external one moves its `src` into
`data-gtmsrc` and has no body, so its URL is listed in `index.md` instead. A tag
can hold several scripts, which is why they are numbered rather than
concatenated; the number is the position in the tag, so a tag whose first script
is external starts at `-1` (its only file may be `html-41-1.js`).

Custom template opcode decompilation is not implemented: a custom template
stored as compiled opcodes is left alone.
