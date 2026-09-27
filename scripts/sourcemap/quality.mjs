// Token-level quality measures for an output source map.
//
// Pure functions over plain data so they test without a parser: tokens are
// `{ kind, value, line, col }` and segments `{ genLine, genCol, srcLine,
// srcCol }` with 0-based lines and UTF-16 columns (source map units).
// `check.mjs` produces both from real files.

// Classify a token for coverage and comparison. `label`/`keyword` follow
// acorn's token types.
export function tokenKind({ label, keyword, value }) {
  if (label === "name" || label === "privateId" || label === "jsxName") return "ident";
  if (keyword) return `kw:${keyword}`;
  if (label === "string" || label === "template" || label === "jsxText") return "string";
  if (label === "num" || label === "bigint") return "num";
  if (label === "regexp") return "regexp";
  return `punct:${value ?? label}`;
}

// Group a kind for the per-kind coverage table.
export function kindGroup(kind) {
  if (kind.startsWith("punct:")) return "punct";
  if (kind.startsWith("kw:")) return "kw";
  return kind;
}

// Segments grouped by generated line, in map order.
function segmentsByLine(segments) {
  const lines = new Map();
  for (const segment of segments) {
    let line = lines.get(segment.genLine);
    if (!line) lines.set(segment.genLine, (line = []));
    line.push(segment);
  }
  return lines;
}

// Measure how well `segments` cover `outputTokens` and whether each mapped
// token lands on a matching token in `inputTokens`.
//
// - `mapped`: output tokens with a segment starting exactly at them.
// - `resolvable`: output tokens a greatest-lower-bound lookup resolves.
// - `compared`: for mapped tokens, the input token at the mapped position is
//   the same token (`same`), an identifier under another name (`renamed`),
//   another token (`different`), or no token start at all (`offToken`,
//   which points at a broken offset rather than a rewrite).
// - `duplicatePositions`: generated positions carrying several segments.
//   Consumers disagree on which one wins; `pick` chooses the first (as
//   `@jridgewell/trace-mapping` does) or the last.
// - `pairs`: non-matching `output kind <- input kind` counts with example
//   positions, most frequent first.
export function analyzeMap({ outputTokens, inputTokens, segments }, { pick = "first", examples = 3 } = {}) {
  const inputAt = new Map(inputTokens.map((token) => [`${token.line}:${token.col}`, token]));
  const lines = segmentsByLine(segments);
  const atPosition = new Map();
  for (const segment of segments) {
    const key = `${segment.genLine}:${segment.genCol}`;
    let list = atPosition.get(key);
    if (!list) atPosition.set(key, (list = []));
    list.push(segment);
  }

  let duplicatePositions = 0;
  for (const list of atPosition.values()) {
    if (list.length > 1) duplicatePositions += 1;
  }

  const byKind = {};
  const compared = { same: 0, renamed: 0, different: 0, offToken: 0 };
  const pairs = new Map();
  let mapped = 0;
  let resolvable = 0;

  for (const token of outputTokens) {
    const group = kindGroup(token.kind);
    const kindStats = (byKind[group] ??= { total: 0, mapped: 0 });
    kindStats.total += 1;

    const line = lines.get(token.line) ?? [];
    if (line.some((segment) => segment.genCol <= token.col)) resolvable += 1;

    const here = atPosition.get(`${token.line}:${token.col}`);
    if (!here) continue;
    mapped += 1;
    kindStats.mapped += 1;

    const segment = pick === "last" ? here[here.length - 1] : here[0];
    const original = inputAt.get(`${segment.srcLine}:${segment.srcCol}`);
    let pairKey;
    if (!original) {
      compared.offToken += 1;
      pairKey = `${token.kind} <- (no token)`;
    } else if (original.kind === token.kind && original.value === token.value) {
      compared.same += 1;
      continue;
    } else if (original.kind === "ident" && token.kind === "ident") {
      compared.renamed += 1;
      continue;
    } else {
      compared.different += 1;
      pairKey = `${token.kind} <- ${original.kind}`;
    }
    let pair = pairs.get(pairKey);
    if (!pair) pairs.set(pairKey, (pair = { count: 0, examples: [] }));
    pair.count += 1;
    if (pair.examples.length < examples) {
      pair.examples.push({
        output: { line: token.line, col: token.col },
        input: { line: segment.srcLine, col: segment.srcCol },
      });
    }
  }

  return {
    outputTokens: outputTokens.length,
    inputTokens: inputTokens.length,
    segments: segments.length,
    mapped,
    resolvable,
    duplicatePositions,
    byKind,
    compared,
    pairs: [...pairs.entries()]
      .map(([key, pair]) => ({ key, ...pair }))
      .sort((a, b) => b.count - a.count),
  };
}

// Sum several `analyzeMap` results (one per unpacked module).
export function mergeResults(results, { examples = 3 } = {}) {
  const total = {
    outputTokens: 0,
    inputTokens: 0,
    segments: 0,
    mapped: 0,
    resolvable: 0,
    duplicatePositions: 0,
    byKind: {},
    compared: { same: 0, renamed: 0, different: 0, offToken: 0 },
    pairs: [],
  };
  const pairs = new Map();
  for (const result of results) {
    for (const field of ["outputTokens", "inputTokens", "segments", "mapped", "resolvable", "duplicatePositions"]) {
      total[field] += result[field];
    }
    for (const [group, stats] of Object.entries(result.byKind)) {
      const merged = (total.byKind[group] ??= { total: 0, mapped: 0 });
      merged.total += stats.total;
      merged.mapped += stats.mapped;
    }
    for (const field of Object.keys(total.compared)) {
      total.compared[field] += result.compared[field];
    }
    for (const pair of result.pairs) {
      let merged = pairs.get(pair.key);
      if (!merged) pairs.set(pair.key, (merged = { key: pair.key, count: 0, examples: [] }));
      merged.count += pair.count;
      for (const example of pair.examples) {
        if (merged.examples.length < examples) merged.examples.push(example);
      }
    }
  }
  total.pairs = [...pairs.values()].sort((a, b) => b.count - a.count);
  return total;
}
