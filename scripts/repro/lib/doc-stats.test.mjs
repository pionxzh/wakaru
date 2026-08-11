import assert from "node:assert/strict";
import test from "node:test";

import {
  extractReadmeReproStat,
  extractWebsiteReproStat,
  findReproStatDrift,
} from "./doc-stats.mjs";

const README_SAMPLE = [
  "## Tested like a compiler",
  "",
  "- **95.0% pattern recovery across 1,234 transpiler × minifier test shapes.**",
  "  Reproduction matrices compile known inputs.",
].join("\n");

const WEBSITE_SAMPLE = [
  '<a class="stat" href="https://example.invalid/stats.json">',
  '  <span class="stat-value">95.0%</span>',
  '  <span class="stat-label">pattern recovery across 1,234 transpiler/minifier test shapes</span>',
  "</a>",
].join("\n");

test("extractReadmeReproStat reads pct and comma-grouped total", () => {
  assert.deepEqual(extractReadmeReproStat(README_SAMPLE), {
    pct: 95.0,
    total: 1234,
  });
});

test("extractReadmeReproStat returns null when the citation is missing", () => {
  assert.equal(extractReadmeReproStat("no stats here"), null);
});

test("extractWebsiteReproStat reads the stat card pair", () => {
  assert.deepEqual(extractWebsiteReproStat(WEBSITE_SAMPLE), {
    pct: 95.0,
    total: 1234,
  });
});

test("findReproStatDrift is empty when both surfaces match", () => {
  const drift = findReproStatDrift(
    { pct: 95.0, total: 1234 },
    { readme: README_SAMPLE, website: WEBSITE_SAMPLE },
  );
  assert.deepEqual(drift, []);
});

test("findReproStatDrift reports a stale surface with both values", () => {
  const drift = findReproStatDrift(
    { pct: 96.2, total: 1300 },
    { readme: README_SAMPLE, website: WEBSITE_SAMPLE },
  );
  assert.equal(drift.length, 2);
  assert.match(drift[0], /^README\.md: cites 95% across 1234/);
  assert.match(drift[0], /records 96\.2% across 1300/);
  assert.match(drift[1], /^website\/index\.html: cites/);
});

test("findReproStatDrift reports an unlocatable citation as drift", () => {
  const drift = findReproStatDrift(
    { pct: 95.0, total: 1234 },
    { readme: "reworded copy", website: WEBSITE_SAMPLE },
  );
  assert.equal(drift.length, 1);
  assert.match(drift[0], /README\.md: could not find/);
});

test("integer pct in stats.json matches a trailing-zero citation", () => {
  // +((yes / total) * 100).toFixed(1) drops the trailing zero, so stats.json
  // records 97 while the copy may say "97.0%"; the comparison is numeric.
  const drift = findReproStatDrift(
    { pct: 95, total: 1234 },
    { readme: README_SAMPLE, website: WEBSITE_SAMPLE },
  );
  assert.deepEqual(drift, []);
});
