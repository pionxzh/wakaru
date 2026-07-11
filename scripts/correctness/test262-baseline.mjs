import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

export const test262BaselineSchemaVersion = 3;

export function createTest262Baseline(report) {
  if (!report.complete) {
    throw new Error("cannot create a Test262 baseline from an incomplete report");
  }
  const outcomes = report.results
    .filter((result) => result.status !== "passed")
    .map(normalizeOutcome)
    .sort(compareOutcome);
  return {
    schemaVersion: test262BaselineSchemaVersion,
    test262: {
      revision: report.options.test262Revision,
    },
    harness: {
      version: report.options.harnessVersion,
    },
    environment: {
      nodeMajor: report.options.nodeMajor,
    },
    producer: report.options.producer,
    wakaru: {
      level: report.options.level,
      caseTimeoutMs: report.options.caseTimeoutMs,
    },
    selection: {
      presets: report.options.presets ?? [],
      paths: report.options.paths,
    },
    totals: baselineTotals(report.totals),
    outcomes,
  };
}

export function loadTest262Baseline(path) {
  if (!existsSync(path)) {
    throw new Error(`missing Test262 baseline ${path}; rerun with --update-baseline`);
  }
  const baseline = JSON.parse(readFileSync(path, "utf8"));
  if (baseline.schemaVersion !== test262BaselineSchemaVersion) {
    throw new Error(
      `unsupported Test262 baseline schema ${baseline.schemaVersion} in ${path}`,
    );
  }
  return baseline;
}

export function saveTest262Baseline(path, baseline) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(baseline, null, 2)}\n`);
}

export function test262BaselineCandidatePath(path) {
  return `${path}.new`;
}

export function acceptTest262BaselineCandidate(path) {
  const candidatePath = test262BaselineCandidatePath(path);
  const candidate = loadTest262Baseline(candidatePath);
  saveTest262Baseline(path, candidate);
  rmSync(candidatePath);
  return { path, candidatePath, baseline: candidate };
}

export function compareTest262Baseline(expected, actual) {
  assertSameIdentity(expected, actual);
  const expectedByKey = new Map(expected.outcomes.map((outcome) => [outcomeKey(outcome), outcome]));
  const actualByKey = new Map(actual.outcomes.map((outcome) => [outcomeKey(outcome), outcome]));
  const newOutcomes = actual.outcomes.filter((outcome) => !expectedByKey.has(outcomeKey(outcome)));
  const unexpectedPasses = expected.outcomes.filter(
    (outcome) => !actualByKey.has(outcomeKey(outcome)),
  );
  const totalsChanged = canonicalJson(expected.totals) !== canonicalJson(actual.totals);
  return {
    clean: newOutcomes.length === 0 && unexpectedPasses.length === 0 && !totalsChanged,
    newOutcomes,
    unexpectedPasses,
    totalsChanged,
    expectedTotals: expected.totals,
    actualTotals: actual.totals,
  };
}

export function applyTest262Baseline(report, { path, update }) {
  const actual = createTest262Baseline(report);
  const candidatePath = test262BaselineCandidatePath(path);
  if (update) {
    saveTest262Baseline(path, actual);
    rmSync(candidatePath, { force: true });
    return {
      path,
      candidatePath: null,
      updated: true,
      clean: true,
      newOutcomes: [],
      unexpectedPasses: [],
      totalsChanged: false,
    };
  }
  let comparison;
  try {
    const expected = loadTest262Baseline(path);
    comparison = compareTest262Baseline(expected, actual);
  } catch (error) {
    saveTest262Baseline(candidatePath, actual);
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`${message}\nCandidate baseline written to ${candidatePath}`, {
      cause: error,
    });
  }
  if (comparison.clean) {
    rmSync(candidatePath, { force: true });
  } else {
    saveTest262Baseline(candidatePath, actual);
  }
  return {
    path,
    candidatePath: comparison.clean ? null : candidatePath,
    updated: false,
    ...comparison,
  };
}

export function validateTest262BaselineOptions(options) {
  if (options.updateBaseline && !options.baseline) {
    throw new Error("--update-baseline requires --baseline <file>");
  }
  if (!options.baseline) {
    return;
  }
  if (Number.isFinite(options.limit)) {
    throw new Error("baseline comparison requires --limit all");
  }
  if (options.rerunFrom) {
    throw new Error("filtered --rerun-from runs cannot compare or update a complete baseline");
  }
  if (!options.presets || options.presets.length !== 1) {
    throw new Error("baseline comparison requires exactly one --preset");
  }
  if (!options.updateBaseline && !existsSync(options.baseline)) {
    throw new Error(`missing Test262 baseline ${options.baseline}; rerun with --update-baseline`);
  }
}

export function fingerprintTest262Outcome(result) {
  const evidence = {
    status: result.status,
    phase: result.phase ?? null,
    reason: result.reason ?? null,
    diagnostic: stableDiagnostic(result.error),
    transformed: hashEvidence(result.transformed),
    decompiled: hashEvidence(result.decompiled),
  };
  return sha256(canonicalJson(evidence));
}

function normalizeOutcome(result) {
  return {
    path: result.path,
    variant: result.variant ?? result.variants?.join(",") ?? "case",
    status: result.status,
    kind: result.reason ?? result.phase ?? result.status,
    fingerprint: fingerprintTest262Outcome(result),
    summary: stableDiagnostic(result.error) || result.reason || result.phase || result.status,
  };
}

function stableDiagnostic(value) {
  if (!value) {
    return "";
  }
  const lines = String(value)
    .replace(/\u001b\[[0-9;]*m/g, "")
    .split(/\r?\n/)
    .filter((line) => !/^\s+at\s/.test(line))
    .map((line) => line.replaceAll("\\", "/"))
    .map((line) => line.replace(/\/(?:private\/)?var\/folders\/[^\s:]+/g, "<tmp>"))
    .map((line) => line.replace(/\/tmp\/[^\s:]+/g, "<tmp>"));
  const typedError = lines.find((line) => /^[A-Za-z_$][\w$]*Error(?::|$)/.test(line));
  if (typedError) {
    return typedError;
  }
  return lines
    .filter((line) => line.trim().length > 0 && !/^\s*\^+\s*$/.test(line))
    .slice(0, 3)
    .join("\n");
}

function hashEvidence(value) {
  if (value == null) {
    return null;
  }
  return sha256(canonicalJson(value));
}

function baselineTotals(totals) {
  return {
    discovered: totals.discovered,
    runnable: totals.runnable,
    skipped: totals.skipped,
    unsupported: totals.unsupported,
    rejected: totals.rejected,
    passed: totals.passed,
    failed: totals.failed,
  };
}

function assertSameIdentity(expected, actual) {
  for (const [label, left, right] of [
    ["Test262 revision", expected.test262, actual.test262],
    ["harness version", expected.harness, actual.harness],
    ["runtime environment", expected.environment, actual.environment],
    ["producer", expected.producer, actual.producer],
    ["Wakaru options", expected.wakaru, actual.wakaru],
    ["selection", expected.selection, actual.selection],
  ]) {
    if (canonicalJson(left) !== canonicalJson(right)) {
      throw new Error(
        `Test262 baseline ${label} mismatch: expected ${canonicalJson(left)}, actual ${canonicalJson(right)}`,
      );
    }
  }
}

function outcomeKey(outcome) {
  return [
    outcome.path,
    outcome.variant,
    outcome.status,
    outcome.kind,
    outcome.fingerprint,
  ].join("\0");
}

function compareOutcome(left, right) {
  return compareCodeUnits(outcomeKey(left), outcomeKey(right));
}

function canonicalJson(value) {
  return JSON.stringify(canonicalize(value));
}

function canonicalize(value) {
  if (Array.isArray(value)) {
    return value.map(canonicalize);
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => compareCodeUnits(left, right))
        .map(([key, child]) => [key, canonicalize(child)]),
    );
  }
  return value;
}

function compareCodeUnits(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}
