// Verifies that human-facing copy citing the reproduction-matrix aggregate
// (README.md, website/index.html) matches stats.json. Regenerating stats.json
// without updating the copy is the drift this catches; collect-stats.mjs
// runs it in both modes so the numbers move together.

const README_PATTERN =
  /\*\*([\d.]+)% pattern recovery across ([\d,]+) transpiler × minifier test shapes\.\*\*/;

const WEBSITE_PATTERN =
  /<span class="stat-value">([\d.]+)%<\/span>\s*<span class="stat-label">pattern recovery across ([\d,]+) transpiler\/minifier test shapes<\/span>/;

function parseCitation(match) {
  return {
    pct: Number.parseFloat(match[1]),
    total: Number.parseInt(match[2].replaceAll(",", ""), 10),
  };
}

export function extractReadmeReproStat(readme) {
  const match = README_PATTERN.exec(readme);
  return match ? parseCitation(match) : null;
}

export function extractWebsiteReproStat(html) {
  const match = WEBSITE_PATTERN.exec(html);
  return match ? parseCitation(match) : null;
}

// Returns human-readable drift messages; empty when every surface cites the
// aggregate correctly. A surface whose citation cannot be located is drift
// too: rewording the copy must update the extractor, never silently skip it.
export function findReproStatDrift(aggregate, surfaces) {
  const checks = [
    ["README.md", extractReadmeReproStat(surfaces.readme)],
    ["website/index.html", extractWebsiteReproStat(surfaces.website)],
  ];

  const drift = [];
  for (const [surface, cited] of checks) {
    if (!cited) {
      drift.push(
        `${surface}: could not find the pattern-recovery citation; ` +
          "update the extractor in scripts/repro/lib/doc-stats.mjs if the copy was reworded",
      );
      continue;
    }
    if (cited.pct !== aggregate.pct || cited.total !== aggregate.total) {
      drift.push(
        `${surface}: cites ${cited.pct}% across ${cited.total} shapes; ` +
          `stats.json records ${aggregate.pct}% across ${aggregate.total}`,
      );
    }
  }
  return drift;
}
