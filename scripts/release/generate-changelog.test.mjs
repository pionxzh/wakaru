import assert from "node:assert/strict";
import test from "node:test";

import {
  filterCommits,
  formatChangelog,
  normalizeRepoUrl,
  parseArgs,
  parseCommit,
  parseGitLog,
  prependChangelog,
  skipCommits,
} from "./generate-changelog.mjs";

test("parseCommit reads conventional type, scope, and description", () => {
  assert.deepEqual(
    parseCommit({
      hash: "abc",
      shortHash: "abc1234",
      subject: "fix(core): preserve binding context",
      body: "",
    }),
    {
      hash: "abc",
      shortHash: "abc1234",
      subject: "fix(core): preserve binding context",
      body: "",
      type: "fix",
      scope: "core",
      description: "preserve binding context",
      breaking: false,
    },
  );
});

test("parseCommit keeps non-conventional commits as other changes", () => {
  const commit = parseCommit({
    hash: "abc",
    shortHash: "abc1234",
    subject: "recover component names from sentry attrs",
    body: "",
  });

  assert.equal(commit.type, "other");
  assert.equal(commit.description, "recover component names from sentry attrs");
});

test("parseGitLog parses git log records separated by control characters", () => {
  const commits = parseGitLog("abc\x00abc1234\x00feat: add cli\x00\x1e\ndef\x00def5678\x00fix: patch bug\x00body\n\x1e\n");

  assert.deepEqual(
    commits.map(({ type, description, shortHash }) => ({ type, description, shortHash })),
    [
      { type: "feat", description: "add cli", shortHash: "abc1234" },
      { type: "fix", description: "patch bug", shortHash: "def5678" },
    ],
  );
});

test("filterCommits defaults to release-note types and preserves breaking changes", () => {
  const commits = [
    parseCommit({ hash: "1", shortHash: "1", subject: "feat: add playground", body: "" }),
    parseCommit({ hash: "2", shortHash: "2", subject: "test: add coverage", body: "" }),
    parseCommit({ hash: "3", shortHash: "3", subject: "refactor!: remove old API", body: "" }),
  ];

  assert.deepEqual(
    filterCommits(commits).map((commit) => commit.subject),
    ["feat: add playground", "refactor!: remove old API"],
  );
  assert.equal(filterCommits(commits, { includeInternal: true }).length, 3);
});

test("skipCommits removes repeated superseded commit hashes", () => {
  const commits = [
    parseCommit({ hash: "abcdef0", shortHash: "abcdef0", subject: "feat: old behavior", body: "" }),
    parseCommit({ hash: "1234567", shortHash: "1234567", subject: "feat: final behavior", body: "" }),
  ];

  assert.deepEqual(
    skipCommits(commits, new Set(["abc"])).map((commit) => commit.subject),
    ["feat: final behavior"],
  );
});

test("formatChangelog groups commits with compare and commit links", () => {
  const output = formatChangelog({
    version: "1.2.0",
    date: "2026-05-25",
    from: "v1.1.0",
    repoUrl: "https://github.com/pionxzh/wakaru",
    commits: [
      parseCommit({
        hash: "abc0000",
        shortHash: "abc0000",
        subject: "feat(core): recover template literals",
        body: "",
      }),
      parseCommit({
        hash: "def1111",
        shortHash: "def1111",
        subject: "fix: preserve strict directives",
        body: "",
      }),
    ],
  });

  assert.match(output, /## \[1\.2\.0\]\(https:\/\/github\.com\/pionxzh\/wakaru\/compare\/v1\.1\.0\.\.\.v1\.2\.0\) \(2026-05-25\)/);
  assert.match(output, /### Features/);
  assert.match(output, /\* \*\*core:\*\* recover template literals \(\[abc0000\]/);
  assert.match(output, /### Bug Fixes/);
});

test("normalizeRepoUrl converts git ssh remotes to https links", () => {
  assert.equal(normalizeRepoUrl("git@github.com:pionxzh/wakaru.git"), "https://github.com/pionxzh/wakaru");
});

test("prependChangelog inserts an entry after the title", () => {
  assert.equal(
    prependChangelog("# Changelog\n\n## 1.1.0\nold\n", "## 1.2.0\nnew\n"),
    "# Changelog\n\n## 1.2.0\nnew\n\n## 1.1.0\nold\n",
  );
});

test("prependChangelog replaces an existing entry for the same version", () => {
  assert.equal(
    prependChangelog("# Changelog\n\n## [1.2.0](link) (2026-05-25)\nstale\n\n## 1.1.0\nold\n", "## 1.2.0\nnew\n", {
      version: "1.2.0",
    }),
    "# Changelog\n\n## 1.2.0\nnew\n\n## 1.1.0\nold\n",
  );
});

test("parseArgs requires a version unless help is requested", () => {
  assert.throws(() => parseArgs([]), /--version is required/);
  assert.equal(parseArgs(["--help"]).help, true);
  assert.equal(parseArgs(["--version", "v1.2.0"]).version, "1.2.0");
});
