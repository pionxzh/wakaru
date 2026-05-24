#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const statusOrder = ["passed", "failed", "unsupported", "rejected", "skipped"];

export function compareReports(before, after) {
  const beforeByPath = resultMap(before);
  const afterByPath = resultMap(after);
  const paths = [...new Set([...beforeByPath.keys(), ...afterByPath.keys()])].sort();
  const transitions = new Map();
  const changed = [];

  for (const path of paths) {
    const from = beforeByPath.get(path);
    const to = afterByPath.get(path);
    const fromKey = resultKey(from);
    const toKey = resultKey(to);
    if (fromKey === toKey) {
      continue;
    }

    const transition = `${fromKey} -> ${toKey}`;
    transitions.set(transition, (transitions.get(transition) ?? 0) + 1);
    changed.push({
      path,
      from: summarizeResult(from),
      to: summarizeResult(to),
    });
  }

  return {
    beforeTotals: before.totals,
    afterTotals: after.totals,
    deltas: diffTotals(before.totals, after.totals),
    transitions: [...transitions.entries()]
      .map(([transition, count]) => ({ transition, count }))
      .sort((a, b) => b.count - a.count || a.transition.localeCompare(b.transition)),
    changed,
  };
}

export function formatComparison(comparison, { details = false } = {}) {
  const lines = [];
  lines.push("# Test262 Report Comparison");
  lines.push("");
  lines.push("## Totals");
  for (const key of ["discovered", "runnable", "skipped", "unsupported", "rejected", "passed", "failed"]) {
    if (
      comparison.beforeTotals[key] === undefined &&
      comparison.afterTotals[key] === undefined &&
      comparison.deltas[key] === undefined
    ) {
      continue;
    }
    lines.push(
      `${key}: ${valueOrZero(comparison.beforeTotals[key])} -> ${valueOrZero(
        comparison.afterTotals[key],
      )} (${formatDelta(comparison.deltas[key])})`,
    );
  }

  lines.push("");
  lines.push("## Transitions");
  if (comparison.transitions.length === 0) {
    lines.push("none");
  } else {
    for (const { transition, count } of comparison.transitions) {
      lines.push(`${count} ${transition}`);
    }
  }

  if (details && comparison.changed.length > 0) {
    lines.push("");
    lines.push("## Changed Paths");
    for (const change of comparison.changed) {
      lines.push(`${change.path}: ${change.from} -> ${change.to}`);
    }
  }

  return `${lines.join("\n")}\n`;
}

function resultMap(report) {
  return new Map((report.results ?? []).map((result) => [result.path, result]));
}

function resultKey(result) {
  if (!result) {
    return "missing";
  }
  if (result.reason) {
    return `${result.status}:${result.reason}`;
  }
  if (result.phase && result.status !== "passed" && result.status !== "skipped") {
    return `${result.status}:${result.phase}`;
  }
  return result.status;
}

function summarizeResult(result) {
  if (!result) {
    return "missing";
  }
  const key = resultKey(result);
  return result.phase && !key.includes(result.phase) ? `${key}:${result.phase}` : key;
}

function diffTotals(beforeTotals = {}, afterTotals = {}) {
  const keys = new Set([...Object.keys(beforeTotals), ...Object.keys(afterTotals), ...statusOrder]);
  const deltas = {};
  for (const key of keys) {
    deltas[key] = valueOrZero(afterTotals[key]) - valueOrZero(beforeTotals[key]);
  }
  return deltas;
}

function valueOrZero(value) {
  return Number.isFinite(value) ? value : 0;
}

function formatDelta(value) {
  const numeric = valueOrZero(value);
  return numeric > 0 ? `+${numeric}` : String(numeric);
}

function usage() {
  return `Usage:
  node scripts/correctness/compare-test262-reports.mjs <before.json> <after.json> [--details]
`;
}

function isMain() {
  return process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
}

if (isMain()) {
  const args = process.argv.slice(2);
  const details = args.includes("--details");
  const files = args.filter((arg) => arg !== "--details");
  if (files.length !== 2) {
    console.error(usage());
    process.exitCode = 1;
  } else {
    const [beforePath, afterPath] = files.map((file) => resolve(file));
    const before = JSON.parse(readFileSync(beforePath, "utf8"));
    const after = JSON.parse(readFileSync(afterPath, "utf8"));
    process.stdout.write(formatComparison(compareReports(before, after), { details }));
  }
}
