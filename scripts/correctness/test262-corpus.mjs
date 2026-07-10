#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  realpathSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));

export const defaultTest262ConfigPath = join(
  repoRoot,
  "scripts",
  "correctness",
  "test262-upstreams.json",
);
export const defaultManagedTest262Root = join(
  repoRoot,
  "target",
  "correctness-tools",
  "test262",
  "vendor",
);

export function loadTest262Upstream(configPath = defaultTest262ConfigPath) {
  const config = JSON.parse(readFileSync(configPath, "utf8"));
  if (config.schemaVersion !== 1) {
    throw new Error(`unsupported Test262 upstream schema ${config.schemaVersion}`);
  }
  const upstream = config.test262;
  if (!upstream || typeof upstream.url !== "string" || upstream.url.length === 0) {
    throw new Error("Test262 upstream URL must be a non-empty string");
  }
  validateRevision(upstream.revision);
  return { schemaVersion: config.schemaVersion, ...upstream };
}

export function inspectTest262Corpus({
  root = defaultManagedTest262Root,
  configPath = defaultTest262ConfigPath,
} = {}) {
  const upstream = loadTest262Upstream(configPath);
  const absoluteRoot = resolve(root);
  if (!existsSync(absoluteRoot)) {
    return corpusStatus(absoluteRoot, upstream, {
      exists: false,
      repository: false,
    });
  }
  if (!isGitRepository(absoluteRoot)) {
    return corpusStatus(absoluteRoot, upstream, {
      exists: true,
      repository: false,
    });
  }

  const head = gitText(absoluteRoot, ["rev-parse", "HEAD"], {
    allowFailure: true,
  });
  const dirty = gitText(absoluteRoot, ["status", "--porcelain"]).length > 0;
  const origin = gitText(absoluteRoot, ["remote", "get-url", "origin"], {
    allowFailure: true,
  });
  return corpusStatus(absoluteRoot, upstream, {
    exists: true,
    repository: true,
    head,
    dirty,
    origin: origin || null,
  });
}

export function setupTest262Corpus({
  root = defaultManagedTest262Root,
  configPath = defaultTest262ConfigPath,
  offline = false,
  force = false,
  upstream = loadTest262Upstream(configPath),
} = {}) {
  const absoluteRoot = resolve(root);
  prepareRepository(absoluteRoot, upstream, { force });

  let status = inspectTest262CorpusWithUpstream(absoluteRoot, upstream);
  if (status.dirty) {
    if (!force) {
      throw new Error(
        `refusing to modify dirty Test262 checkout at ${absoluteRoot}; clean it or rerun with --force`,
      );
    }
    rmSync(absoluteRoot, { recursive: true, force: true });
    prepareRepository(absoluteRoot, upstream, { force: false });
    status = inspectTest262CorpusWithUpstream(absoluteRoot, upstream);
  }

  if (status.origin !== upstream.url) {
    if (status.origin && !force) {
      throw new Error(
        `Test262 checkout origin is ${status.origin}, expected ${upstream.url}; rerun with --force to replace it`,
      );
    }
    if (status.origin) {
      git(absoluteRoot, ["remote", "set-url", "origin", upstream.url]);
    } else {
      git(absoluteRoot, ["remote", "add", "origin", upstream.url]);
    }
  }

  if (status.head !== upstream.revision) {
    if (offline) {
      const available = git(absoluteRoot, ["cat-file", "-e", `${upstream.revision}^{commit}`], {
        allowFailure: true,
      }).status === 0;
      if (!available) {
        throw new Error(
          `pinned Test262 revision ${upstream.revision} is unavailable offline at ${absoluteRoot}`,
        );
      }
      git(absoluteRoot, ["checkout", "--detach", upstream.revision]);
    } else {
      git(absoluteRoot, ["fetch", "--depth=1", "origin", upstream.revision]);
      git(absoluteRoot, ["checkout", "--detach", "FETCH_HEAD"]);
    }
  }

  status = inspectTest262CorpusWithUpstream(absoluteRoot, upstream);
  if (!status.ready) {
    throw new Error(
      `Test262 setup ended at ${status.head ?? "no revision"}, expected ${upstream.revision}`,
    );
  }
  return status;
}

export function updateTest262Pin({
  revision,
  root = defaultManagedTest262Root,
  configPath = defaultTest262ConfigPath,
  offline = false,
  force = false,
} = {}) {
  validateRevision(revision);
  const current = loadTest262Upstream(configPath);
  const upstream = { ...current, revision };
  const status = setupTest262Corpus({
    root,
    configPath,
    offline,
    force,
    upstream,
  });
  const config = {
    schemaVersion: current.schemaVersion,
    test262: {
      url: upstream.url,
      revision: upstream.revision,
    },
  };
  writeFileSync(configPath, `${JSON.stringify(config, null, 2)}\n`);
  return status;
}

export function assertPinnedTest262Corpus({
  root = defaultManagedTest262Root,
  configPath = defaultTest262ConfigPath,
} = {}) {
  const status = inspectTest262Corpus({ root, configPath });
  if (!status.ready || status.dirty) {
    const reason = !status.exists
      ? "is missing"
      : !status.repository
        ? "is not a git checkout"
        : status.dirty
          ? "is dirty"
          : `is at ${status.head}, expected ${status.configuredRevision}`;
    throw new Error(
      `managed Test262 checkout ${reason}; run \`node scripts/correctness/test262-corpus.mjs setup\``,
    );
  }
  return status;
}

export function readTest262Revision(root) {
  const absoluteRoot = resolve(root);
  if (!isGitRepository(absoluteRoot)) {
    return null;
  }
  return gitText(absoluteRoot, ["rev-parse", "HEAD"], { allowFailure: true }) || null;
}

function corpusStatus(root, upstream, actual) {
  return {
    root,
    configuredUrl: upstream.url,
    configuredRevision: upstream.revision,
    exists: actual.exists,
    repository: actual.repository,
    head: actual.head ?? null,
    dirty: actual.dirty ?? false,
    origin: actual.origin ?? null,
    ready:
      actual.repository === true &&
      actual.head === upstream.revision &&
      actual.dirty !== true &&
      actual.origin === upstream.url,
  };
}

function inspectTest262CorpusWithUpstream(root, upstream) {
  if (!existsSync(root) || !isGitRepository(root)) {
    return corpusStatus(root, upstream, {
      exists: existsSync(root),
      repository: false,
    });
  }
  const origin = gitText(root, ["remote", "get-url", "origin"], {
    allowFailure: true,
  });
  return corpusStatus(root, upstream, {
    exists: true,
    repository: true,
    head: gitText(root, ["rev-parse", "HEAD"], { allowFailure: true }) || null,
    dirty: gitText(root, ["status", "--porcelain"]).length > 0,
    origin: origin || null,
  });
}

function prepareRepository(root, upstream, { force }) {
  if (existsSync(root) && !isGitRepository(root)) {
    const nonEmpty = readdirSync(root).length > 0;
    if (nonEmpty && !force) {
      throw new Error(
        `refusing to replace non-git Test262 directory at ${root}; rerun with --force`,
      );
    }
    if (force) {
      rmSync(root, { recursive: true, force: true });
    }
  }
  if (!existsSync(root)) {
    mkdirSync(dirname(root), { recursive: true });
    mkdirSync(root, { recursive: true });
  }
  if (!isGitRepository(root)) {
    git(root, ["init"]);
    git(root, ["remote", "add", "origin", upstream.url]);
  }
}

function isGitRepository(root) {
  if (!existsSync(root)) {
    return false;
  }
  const result = spawnSync("git", ["-C", root, "rev-parse", "--show-toplevel"], {
    encoding: "utf8",
  });
  return (
    result.status === 0 &&
    realpathSync(result.stdout.trim()) === realpathSync(root)
  );
}

function gitText(root, args, options = {}) {
  const result = git(root, args, options);
  if (result.status !== 0) {
    return "";
  }
  return result.stdout.trim();
}

function git(root, args, { allowFailure = false } = {}) {
  const result = spawnSync("git", ["-C", root, ...args], {
    encoding: "utf8",
    maxBuffer: 10 * 1024 * 1024,
  });
  if (result.error) {
    throw new Error(`git ${args.join(" ")} failed: ${result.error.message}`);
  }
  if (result.status !== 0 && !allowFailure) {
    throw new Error(
      `git ${args.join(" ")} failed with exit ${result.status}\n${result.stderr || result.stdout}`,
    );
  }
  return result;
}

function validateRevision(revision) {
  if (typeof revision !== "string" || !/^[0-9a-f]{40}$/i.test(revision)) {
    throw new Error("Test262 revision must be a full 40-character commit SHA");
  }
}

function parseArgs(argv) {
  const command = argv[0];
  if (!command || !["setup", "status", "update"].includes(command)) {
    throw new Error(usage());
  }
  const options = {
    command,
    root: defaultManagedTest262Root,
    configPath: defaultTest262ConfigPath,
    offline: false,
    force: false,
    json: false,
    revision: null,
  };
  for (let index = 1; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--root") {
      options.root = resolve(readValue(argv, ++index, arg));
    } else if (arg === "--config") {
      options.configPath = resolve(readValue(argv, ++index, arg));
    } else if (arg === "--revision") {
      options.revision = readValue(argv, ++index, arg);
    } else if (arg === "--offline") {
      options.offline = true;
    } else if (arg === "--force") {
      options.force = true;
    } else if (arg === "--json") {
      options.json = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  return options;
}

function readValue(argv, index, option) {
  const value = argv[index];
  if (!value || value.startsWith("-")) {
    throw new Error(`${option} requires a value`);
  }
  return value;
}

function usage() {
  return `Usage:
  node scripts/correctness/test262-corpus.mjs setup [--offline] [--force]
  node scripts/correctness/test262-corpus.mjs status [--json]
  node scripts/correctness/test262-corpus.mjs update --revision <full-sha> [--force]

Options:
  --root <dir>       Override the managed checkout directory
  --config <file>    Override the tracked upstream manifest
  --offline          Require the pinned commit to exist locally
  --force            Explicitly replace dirty or mismatched fixture state
  --json             Print machine-readable status
`;
}

function printStatus(status, json) {
  if (json) {
    process.stdout.write(`${JSON.stringify(status, null, 2)}\n`);
    return;
  }
  console.log(`root: ${status.root}`);
  console.log(`configured revision: ${status.configuredRevision}`);
  console.log(`checkout revision: ${status.head ?? "missing"}`);
  console.log(`dirty: ${status.dirty}`);
  console.log(`ready: ${status.ready}`);
}

function isMain() {
  return process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
}

if (isMain()) {
  try {
    const options = parseArgs(process.argv.slice(2));
    let status;
    if (options.command === "setup") {
      status = setupTest262Corpus(options);
    } else if (options.command === "update") {
      if (!options.revision) {
        throw new Error("update requires --revision <full-sha>");
      }
      status = updateTest262Pin(options);
    } else {
      status = inspectTest262Corpus(options);
    }
    printStatus(status, options.json);
    if (options.command === "status" && !status.ready) {
      process.exitCode = 1;
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
