import assert from "node:assert/strict";
import { test } from "node:test";
import { analyzeMap, kindGroup, mergeResults, tokenKind, tokenValue } from "./quality.mjs";

const token = (kind, value, line, col, endLine = line, endCol = col + String(value).length) => ({
  kind,
  value,
  line,
  col,
  endLine,
  endCol,
});
const segment = (genLine, genCol, srcLine, srcCol) => ({ genLine, genCol, srcLine, srcCol });

test("tokenKind separates identifiers, keywords, literals, and punctuation", () => {
  assert.equal(tokenKind({ label: "name", value: "a" }), "ident");
  assert.equal(tokenKind({ label: "jsxName", value: "div" }), "ident");
  assert.equal(tokenKind({ label: "const", keyword: "const", value: "const" }), "kw:const");
  assert.equal(tokenKind({ label: "template", value: "x" }), "string");
  assert.equal(tokenKind({ label: "num", value: 1 }), "num");
  assert.equal(tokenKind({ label: "(", value: undefined }), "punct:(");
  assert.equal(kindGroup("punct:("), "punct");
  assert.equal(kindGroup("kw:if"), "kw");
  assert.equal(kindGroup("ident"), "ident");
});

test("tokenValue makes identical regexps compare equal", () => {
  // acorn gives each regexp token its own object value.
  const regexp = () => ({ pattern: "\\s+", flags: "g", value: /\s+/g });
  assert.equal(tokenValue({ label: "regexp", value: regexp() }), tokenValue({ label: "regexp", value: regexp() }));
  assert.notEqual(
    tokenValue({ label: "regexp", value: regexp() }),
    tokenValue({ label: "regexp", value: { pattern: "\\s+", flags: "", value: /\s+/ } }),
  );
  assert.equal(tokenValue({ label: "name", value: "a" }), "a");

  const result = analyzeMap({
    outputTokens: [token("regexp", tokenValue({ label: "regexp", value: regexp() }), 0, 0)],
    inputTokens: [token("regexp", tokenValue({ label: "regexp", value: regexp() }), 0, 4)],
    segments: [segment(0, 0, 0, 4)],
  });
  assert.equal(result.compared.same, 1);
});

test("analyzeMap classifies mapped tokens against the input", () => {
  // input:  var a = !0;     output: const b = true;
  const inputTokens = [
    token("kw:var", "var", 0, 0),
    token("ident", "a", 0, 4),
    token("punct:=", "=", 0, 6),
    token("punct:!", "!", 0, 8),
    token("num", 0, 0, 9),
  ];
  const outputTokens = [
    token("kw:const", "const", 0, 0),
    token("ident", "b", 0, 6),
    token("punct:=", "=", 0, 8),
    token("kw:true", "true", 0, 10),
    token("punct:;", ";", 0, 14),
  ];
  const segments = [
    segment(0, 0, 0, 0), // const <- var
    segment(0, 6, 0, 4), // b <- a
    segment(0, 10, 0, 8), // true <- !
    segment(0, 12, 0, 5), // inside `true`: resolves the `;` lookup, lands off any input token start
  ];
  const result = analyzeMap({ outputTokens, inputTokens, segments });

  assert.equal(result.mapped, 3);
  assert.equal(result.resolvable, 5);
  assert.deepEqual(result.compared, { same: 0, renamed: 1, different: 2, tokenEnd: 0, offToken: 0 });
  assert.deepEqual(result.byKind.punct, { total: 2, mapped: 0 });
  assert.deepEqual(
    result.pairs.map(({ key, count }) => [key, count]),
    [
      ["kw:const <- kw:var", 1],
      ["kw:true <- punct:!", 1],
    ],
  );
  assert.deepEqual(result.pairs[1].examples, [{ output: { line: 0, col: 10 }, input: { line: 0, col: 8 } }]);
});

test("analyzeMap reports duplicates and resolves them by the chosen pick", () => {
  // A parent and its first child share an output column but point at
  // different input tokens.
  const inputTokens = [token("num", 1, 0, 0), token("ident", "a", 0, 6)];
  const outputTokens = [token("ident", "a", 0, 0)];
  const segments = [segment(0, 0, 0, 0), segment(0, 0, 0, 6)];

  const first = analyzeMap({ outputTokens, inputTokens, segments });
  assert.equal(first.duplicatePositions, 1);
  assert.deepEqual(first.compared, { same: 0, renamed: 0, different: 1, tokenEnd: 0, offToken: 0 });

  const last = analyzeMap({ outputTokens, inputTokens, segments }, { pick: "last" });
  assert.deepEqual(last.compared, { same: 1, renamed: 0, different: 0, tokenEnd: 0, offToken: 0 });
});

test("analyzeMap flags mappings that land off every input token", () => {
  const result = analyzeMap({
    outputTokens: [token("ident", "a", 0, 0)],
    inputTokens: [token("ident", "a", 0, 0)],
    segments: [segment(0, 0, 0, 3)],
  });
  assert.equal(result.compared.offToken, 1);
  assert.equal(result.pairs[0].key, "ident <- (no token)");
});

test("analyzeMap separates mappings at the end of an input token", () => {
  // input:  f(a.b\n)     output: f(a.b);
  // The output `;` maps to the end of `b`, where no input token starts.
  const inputTokens = [
    token("ident", "f", 0, 0),
    token("punct:(", "(", 0, 1),
    token("ident", "a", 0, 2),
    token("punct:.", ".", 0, 3),
    token("ident", "b", 0, 4),
    token("punct:)", ")", 1, 0),
  ];
  const result = analyzeMap({
    outputTokens: [token("ident", "b", 0, 4), token("punct:)", ")", 0, 5), token("punct:;", ";", 0, 6)],
    inputTokens,
    segments: [segment(0, 4, 0, 4), segment(0, 5, 1, 0), segment(0, 6, 0, 5)],
  });
  assert.deepEqual(result.compared, { same: 2, renamed: 0, different: 0, tokenEnd: 1, offToken: 0 });
  assert.deepEqual(
    result.pairs.map(({ key, count }) => [key, count]),
    [["punct:; <- (end of ident)", 1]],
  );
});

test("analyzeMap counts a position that ends one token and starts another as a start", () => {
  // input: a)   `a` ends where `)` starts.
  const result = analyzeMap({
    outputTokens: [token("punct:)", ")", 0, 0)],
    inputTokens: [token("ident", "a", 0, 0), token("punct:)", ")", 0, 1)],
    segments: [segment(0, 0, 0, 1)],
  });
  assert.deepEqual(result.compared, { same: 1, renamed: 0, different: 0, tokenEnd: 0, offToken: 0 });
});

test("mergeResults sums counts and merges pairs across files", () => {
  const one = analyzeMap({
    outputTokens: [token("kw:if", "if", 0, 0)],
    inputTokens: [token("ident", "x", 0, 0)],
    segments: [segment(0, 0, 0, 0)],
  });
  const two = analyzeMap({
    outputTokens: [token("kw:if", "if", 0, 0), token("ident", "y", 1, 0)],
    inputTokens: [token("ident", "x", 0, 0), token("ident", "y", 1, 0)],
    segments: [segment(0, 0, 0, 0), segment(1, 0, 1, 0)],
  });
  const total = mergeResults([one, two], { examples: 1 });

  assert.equal(total.outputTokens, 3);
  assert.equal(total.mapped, 3);
  assert.deepEqual(total.compared, { same: 1, renamed: 0, different: 2, tokenEnd: 0, offToken: 0 });
  assert.deepEqual(total.byKind.kw, { total: 2, mapped: 2 });
  assert.equal(total.pairs.length, 1);
  assert.equal(total.pairs[0].count, 2);
  assert.equal(total.pairs[0].examples.length, 1);
});
