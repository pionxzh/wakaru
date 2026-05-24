#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const defaultTypes = new Set(["feat", "fix", "perf"]);
const typeTitles = new Map([
  ["feat", "Features"],
  ["fix", "Bug Fixes"],
  ["perf", "Performance"],
  ["docs", "Documentation"],
  ["test", "Tests"],
  ["refactor", "Refactoring"],
  ["ci", "Continuous Integration"],
  ["build", "Build System"],
  ["chore", "Chores"],
  ["revert", "Reverts"],
  ["other", "Other Changes"],
]);
const typeOrder = ["feat", "fix", "perf", "docs", "test", "refactor", "ci", "build", "chore", "revert", "other"];

export function parseArgs(argv) {
  const options = {
    from: null,
    to: "HEAD",
    version: null,
    date: new Date().toISOString().slice(0, 10),
    repoUrl: null,
    includeInternal: false,
    skips: new Set(),
    output: null,
    prepend: null,
    help: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--from") {
      options.from = readValue(argv, ++i, arg);
    } else if (arg === "--to") {
      options.to = readValue(argv, ++i, arg);
    } else if (arg === "--version") {
      options.version = readValue(argv, ++i, arg).replace(/^v/, "");
    } else if (arg === "--date") {
      options.date = readValue(argv, ++i, arg);
    } else if (arg === "--repo") {
      options.repoUrl = readValue(argv, ++i, arg).replace(/\/$/, "");
    } else if (arg === "--include-internal") {
      options.includeInternal = true;
    } else if (arg === "--skip") {
      options.skips.add(readValue(argv, ++i, arg));
    } else if (arg === "--output") {
      options.output = readValue(argv, ++i, arg);
    } else if (arg === "--prepend") {
      options.prepend = readValue(argv, ++i, arg);
    } else if (arg === "--help" || arg === "-h") {
      options.help = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }

  if (!options.help && !options.version) {
    throw new Error("--version is required");
  }

  return options;
}

export function parseGitLog(output) {
  if (!output.trim()) {
    return [];
  }

  return output
    .split("\x1e")
    .filter((entry) => entry.trim())
    .map((entry) => {
      const [hash, shortHash, subject, body = ""] = entry.trim().split("\x00");
      return parseCommit({ hash, shortHash, subject, body });
    });
}

export function parseCommit(commit) {
  const match = /^(?<type>[a-z]+)(?:\((?<scope>[^)]+)\))?(?<breaking>!)?:\s+(?<description>.+)$/.exec(
    commit.subject,
  );
  const breaking = Boolean(match?.groups?.breaking) || /\n?BREAKING[ -]CHANGE:/i.test(commit.body ?? "");

  if (!match) {
    return {
      ...commit,
      type: "other",
      scope: null,
      description: commit.subject,
      breaking,
    };
  }

  return {
    ...commit,
    type: match.groups.type,
    scope: match.groups.scope ?? null,
    description: match.groups.description,
    breaking,
  };
}

export function filterCommits(commits, { includeInternal = false } = {}) {
  const typeFiltered = includeInternal
    ? commits
    : commits.filter((commit) => defaultTypes.has(commit.type) || commit.type === "other" || commit.breaking);
  return typeFiltered;
}

export function skipCommits(commits, skips = new Set()) {
  if (skips.size === 0) {
    return commits;
  }
  return commits.filter(
    (commit) => ![commit.hash, commit.shortHash].some((hash) => [...skips].some((skip) => hash.startsWith(skip))),
  );
}

export function formatChangelog({ version, date, from, to = `v${version}`, repoUrl, commits }) {
  const lines = [];
  const versionRef = `v${version}`;
  const heading = repoUrl
    ? `## [${version}](${repoUrl}/compare/${from}...${versionRef}) (${date})`
    : `## ${version} (${date})`;

  lines.push(heading);
  lines.push("");

  const breaking = commits.filter((commit) => commit.breaking);
  if (breaking.length > 0) {
    lines.push("### Breaking Changes");
    lines.push("");
    for (const commit of breaking) {
      lines.push(formatBullet(commit, repoUrl));
    }
    lines.push("");
  }

  for (const type of typeOrder) {
    const typed = commits.filter((commit) => commit.type === type && !commit.breaking);
    if (typed.length === 0) {
      continue;
    }
    lines.push(`### ${typeTitles.get(type) ?? type}`);
    lines.push("");
    for (const commit of typed) {
      lines.push(formatBullet(commit, repoUrl));
    }
    lines.push("");
  }

  const unknownTypes = [...new Set(commits.map((commit) => commit.type))]
    .filter((type) => !typeOrder.includes(type))
    .sort();
  for (const type of unknownTypes) {
    const typed = commits.filter((commit) => commit.type === type && !commit.breaking);
    if (typed.length === 0) {
      continue;
    }
    lines.push(`### ${typeTitles.get(type) ?? titleCase(type)}`);
    lines.push("");
    for (const commit of typed) {
      lines.push(formatBullet(commit, repoUrl));
    }
    lines.push("");
  }

  if (lines.at(-1) === "") {
    lines.pop();
  }

  return `${lines.join("\n")}\n`;
}

export function prependChangelog(existing, entry, { version = null } = {}) {
  const normalizedEntry = entry.trimEnd();
  const deduped = version ? removeVersionEntry(existing, version) : existing;
  const lines = deduped.split(/\r?\n/);
  const firstBodyLine = lines.findIndex((line, index) => index > 0 && line.trim() !== "");

  if (lines[0]?.startsWith("# ") && firstBodyLine !== -1) {
    const header = lines.slice(0, firstBodyLine).join("\n").trimEnd();
    const rest = lines.slice(firstBodyLine).join("\n").trimStart();
    return `${header}\n\n${normalizedEntry}\n\n${rest}`;
  }
  if (lines[0]?.startsWith("# ")) {
    return `${deduped.trimEnd()}\n\n${normalizedEntry}\n`;
  }
  return `${normalizedEntry}\n\n${deduped.trimStart()}`;
}

function removeVersionEntry(changelog, version) {
  const escaped = escapeRegex(version);
  const heading = String.raw`## (?:\[` + escaped + String.raw`\]|` + escaped + String.raw`)(?:\([^)]*\))? \([^)]*\)`;
  return changelog.replace(new RegExp(String.raw`\n?${heading}[\s\S]*?(?=\n## |\s*$)`), "\n");
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function normalizeRepoUrl(remote) {
  const trimmed = remote.trim().replace(/\.git$/, "");
  const sshMatch = /^git@([^:]+):(.+)$/.exec(trimmed);
  if (sshMatch) {
    return `https://${sshMatch[1]}/${sshMatch[2]}`;
  }
  return trimmed;
}

function formatBullet(commit, repoUrl) {
  const scope = commit.scope ? `**${commit.scope}:** ` : "";
  const description = `${scope}${commit.description}`;
  if (!repoUrl) {
    return `* ${description} (${commit.shortHash})`;
  }
  return `* ${description} ([${commit.shortHash}](${repoUrl}/commit/${commit.hash}))`;
}

function titleCase(value) {
  return value
    .split("-")
    .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

function readValue(argv, index, flag) {
  const value = argv[index];
  if (!value || value.startsWith("-")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" });
}

function latestTag() {
  return git(["describe", "--tags", "--abbrev=0"]).trim();
}

function currentRepoUrl() {
  return normalizeRepoUrl(git(["config", "--get", "remote.origin.url"]));
}

function gitLog({ from, to }) {
  const range = `${from}..${to}`;
  return git(["log", "--no-merges", "--format=%H%x00%h%x00%s%x00%b%x1e", range]);
}

export function usage() {
  return `Usage:
  node scripts/release/generate-changelog.mjs --version <version> [--from <tag>] [--to <ref>] [--date YYYY-MM-DD]

Options:
  --from <tag>           Previous release tag. Defaults to latest reachable tag.
  --to <ref>             Release head. Defaults to HEAD.
  --repo <url>           Repository URL for compare and commit links. Defaults to origin.
  --include-internal     Include docs, tests, refactors, chores, CI, and build commits.
  --skip <hash>          Omit a superseded commit. Can be repeated.
  --output <path>        Write generated Markdown to a file instead of stdout.
  --prepend <path>       Insert generated Markdown after the changelog title.
`;
}

function isMain() {
  return process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
}

if (isMain()) {
  try {
    const options = parseArgs(process.argv.slice(2));
    if (options.help) {
      process.stdout.write(usage());
    } else {
      const from = options.from ?? latestTag();
      const repoUrl = options.repoUrl ?? currentRepoUrl();
      const commits = skipCommits(filterCommits(parseGitLog(gitLog({ from, to: options.to })), options), options.skips);
      const markdown = formatChangelog({
        version: options.version,
        date: options.date,
        from,
        to: options.to,
        repoUrl,
        commits,
      });
      if (options.output) {
        writeFileSync(options.output, markdown);
      } else if (options.prepend) {
        const path = resolve(options.prepend);
        writeFileSync(path, prependChangelog(readFileSync(path, "utf8"), markdown, { version: options.version }));
      } else {
        process.stdout.write(markdown);
      }
    }
  } catch (error) {
    console.error(error.message);
    console.error("");
    console.error(usage());
    process.exitCode = 1;
  }
}
