import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { constants, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const workflowPath = join(repoRoot, ".github/workflows/rust-release.yml");
const launcherPath = join(repoRoot, "npm/bin/wakaru");
const platformsDir = join(repoRoot, "npm/platforms");

const workflow = readFileSync(workflowPath, "utf8");
const launcher = readFileSync(launcherPath, "utf8");
const mainPackage = JSON.parse(readFileSync(join(repoRoot, "npm/package.json"), "utf8"));

function sorted(values) {
  return [...values].sort();
}

function platformPackageDirectories() {
  return readdirSync(platformsDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .filter((name) => name.startsWith("cli-"));
}

test("release workflow publishes every npm platform package", () => {
  const matrixPackages = [...workflow.matchAll(/^\s+npm-pkg:\s*(\S+)\s*$/gm)].map(
    (match) => match[1],
  );
  const publishLoop = workflow.match(/for pkg in ([^;\n]+); do/);
  assert.ok(publishLoop, "release workflow must contain the platform package publish loop");
  const publishedPackages = publishLoop[1].trim().split(/\s+/);

  const optionalPackages = Object.keys(mainPackage.optionalDependencies).map((name) =>
    name.replace("@wakaru/", ""),
  );
  const launcherPackages = [...launcher.matchAll(/@wakaru\/(cli-[^/"']+)\//g)].map(
    (match) => match[1],
  );
  const packageDirectories = platformPackageDirectories();

  const expected = sorted(packageDirectories);
  assert.deepEqual(sorted(matrixPackages), expected, "build matrix and platform directories differ");
  assert.deepEqual(sorted(publishedPackages), expected, "publish loop and platform directories differ");
  assert.deepEqual(sorted(optionalPackages), expected, "optional dependencies and platform directories differ");
  assert.deepEqual(sorted(launcherPackages), expected, "launcher map and platform directories differ");
});

test("platform package metadata matches its directory", () => {
  for (const directory of platformPackageDirectories()) {
    const match = /^cli-(darwin|linux|win32)-(arm64|x64)$/.exec(directory);
    assert.ok(match, `unrecognized platform package directory: ${directory}`);
    const [, platform, architecture] = match;
    const manifest = JSON.parse(readFileSync(join(platformsDir, directory, "package.json"), "utf8"));
    const binary = platform === "win32" ? "wakaru.exe" : "wakaru";

    assert.equal(manifest.name, `@wakaru/${directory}`);
    assert.deepEqual(manifest.os, [platform]);
    assert.deepEqual(manifest.cpu, [architecture]);
    assert.deepEqual(manifest.bin, { wakaru: `./${binary}` });
    assert.deepEqual(manifest.files, [binary]);
  }
});

test("unsupported-platform error lists every supported platform", () => {
  const script = `
    Object.defineProperty(process, "platform", { value: "unsupported" });
    Object.defineProperty(process, "arch", { value: "unknown" });
    require(${JSON.stringify(launcherPath)});
  `;
  const result = spawnSync(process.execPath, ["-e", script], { encoding: "utf8" });

  assert.equal(result.status, 1);
  const supported = result.stderr.match(/Supported: ([^.]+)\./);
  assert.ok(supported, `missing supported-platform list in stderr: ${result.stderr}`);

  const actual = supported[1].split(", ");
  const expected = platformPackageDirectories().map((name) => name.replace("cli-", ""));
  assert.deepEqual(sorted(actual), sorted(expected));
});

// Installs a copy of the launcher next to a fake platform package whose binary
// is the given shell script, and runs the launcher.
function runLauncherWithFakeBinary(script) {
  const root = mkdtempSync(join(tmpdir(), "wakaru-launcher-"));
  try {
    mkdirSync(join(root, "bin"));
    copyFileSync(launcherPath, join(root, "bin/wakaru"));
    const packageDir = join(root, `node_modules/@wakaru/cli-${process.platform}-${process.arch}`);
    mkdirSync(packageDir, { recursive: true });
    writeFileSync(join(packageDir, "wakaru"), `#!/bin/sh\n${script}\n`, { mode: 0o755 });
    return spawnSync(process.execPath, [join(root, "bin/wakaru")], { encoding: "utf8" });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

const posixSkip = process.platform === "win32" ? "needs POSIX signals and a shell-script binary" : false;

test("launcher passes the binary's exit status through", { skip: posixSkip }, () => {
  const result = runLauncherWithFakeBinary("exit 3");

  assert.equal(result.signal, null, result.stderr);
  assert.equal(result.status, 3, result.stderr);
});

for (const signal of ["SIGABRT", "SIGTERM"]) {
  test(`launcher dies of the ${signal} that killed the binary`, { skip: posixSkip }, () => {
    const result = runLauncherWithFakeBinary(`kill -${signal.slice(3)} $$`);

    assert.equal(result.signal, signal, `status ${result.status}: ${result.stderr}`);
  });
}

test("launcher exits 128 + signal when it survives the binary's signal", { skip: posixSkip }, () => {
  // Node ignores SIGPIPE, so re-raising it does not end the launcher.
  const result = runLauncherWithFakeBinary("kill -PIPE $$");

  assert.equal(result.signal, null, result.stderr);
  assert.equal(result.status, 128 + constants.signals.SIGPIPE, result.stderr);
});
