import { spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

const MARKER = ".installed";
const RENAME_ATTEMPTS = 8;

function gitCommonDir(root) {
  const result = spawnSync("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], {
    cwd: root,
    encoding: "utf8",
  });
  return result.status === 0 ? result.stdout.trim() : null;
}

// Every worktree of one clone shares a single tool cache: the main checkout's
// target/repro-tools, found through the git common directory. Installs are
// keyed by exact versions and lockfile hashes, so sharing them is safe, and a
// fresh worktree skips minutes of npm installs. WAKARU_REPRO_TOOLS_DIR
// overrides the location; outside a regular clone the cache stays in `root`.
export function resolveReproToolsRoot(root, { env = process.env, commonDir = gitCommonDir } = {}) {
  if (env.WAKARU_REPRO_TOOLS_DIR) {
    return resolve(env.WAKARU_REPRO_TOOLS_DIR);
  }
  const common = commonDir(root);
  const mainCheckout = common && basename(common) === ".git" ? dirname(common) : root;
  return join(mainCheckout, "target", "repro-tools");
}

// The directory name carries a hash of the marker, so worktrees that pin
// different contents for the same tool name (e.g. a branch that edits a
// lockfile) get separate installs instead of replacing each other's.
export function nodeToolDir(toolsRoot, name, markerText) {
  const digest = createHash("sha256").update(markerText).digest("hex").slice(0, 12);
  return join(toolsRoot, `${name}-${digest}`);
}

function hasMarker(dir, markerText) {
  const marker = join(dir, MARKER);
  return existsSync(marker) && readFileSync(marker, "utf8") === markerText;
}

function siblingPath(dir, kind) {
  return `${dir}.${kind}-${process.pid}-${randomBytes(4).toString("hex")}`;
}

// Tool directories under the repro tool cache are shared by every matrix and
// test process in every worktree, and `node --test` runs test files in parallel workers, so two
// processes routinely install the same tool at the same time. Each installer
// populates a private staging directory next to the target and renames it into
// place: a concurrent process never observes a half-written tree, and a
// complete install is never deleted from under a process that already returned
// it. When another installer wins the rename, the loser discards its copy and
// uses the winner's. A stale directory (different marker) is moved aside by
// rename before removal for the same reason.
export function installNodeTool(dir, markerText, populate, { refresh = false } = {}) {
  if (!refresh && hasMarker(dir, markerText)) {
    return dir;
  }
  mkdirSync(dirname(dir), { recursive: true });
  const staging = siblingPath(dir, "staging");
  mkdirSync(staging);
  try {
    populate(staging);
    writeFileSync(join(staging, MARKER), markerText);
  } catch (error) {
    rmSync(staging, { recursive: true, force: true });
    throw error;
  }
  // A refresh replaces the install it found, but a concurrent installer that
  // completes first already delivered a fresh copy; accept it after that.
  let acceptExisting = !refresh;
  let lastError;
  for (let attempt = 0; attempt < RENAME_ATTEMPTS; attempt++) {
    if (acceptExisting && hasMarker(dir, markerText)) {
      rmSync(staging, { recursive: true, force: true });
      return dir;
    }
    if (existsSync(dir)) {
      const aside = siblingPath(dir, "stale");
      try {
        renameSync(dir, aside);
      } catch (error) {
        // Another installer moved it aside first.
        if (error.code !== "ENOENT") throw error;
      }
      rmSync(aside, { recursive: true, force: true });
    }
    try {
      renameSync(staging, dir);
      return dir;
    } catch (error) {
      // The target reappeared: another installer renamed its copy in between.
      if (!["ENOTEMPTY", "EEXIST", "EPERM"].includes(error.code)) throw error;
      lastError = error;
      acceptExisting = true;
    }
  }
  rmSync(staging, { recursive: true, force: true });
  throw new Error(`could not install ${dir} after ${RENAME_ATTEMPTS} attempts: ${lastError.message}`);
}
