import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { installNodeTool, nodeToolDir, resolveReproToolsRoot } from "./node-tool.mjs";

const moduleUrl = new URL("./node-tool.mjs", import.meta.url).href;

function withRoot(t) {
  const root = mkdtempSync(join(tmpdir(), "wakaru-node-tool-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

function payload(dir) {
  return readFileSync(join(dir, "payload.txt"), "utf8");
}

test("a fresh install lands in place with its marker and no staging leftovers", (t) => {
  const root = withRoot(t);
  const dir = join(root, "tool");
  let populated = 0;
  const populate = (staging) => {
    populated++;
    assert.notEqual(staging, dir);
    writeFileSync(join(staging, "payload.txt"), "first");
  };
  assert.equal(installNodeTool(dir, "marker-a", populate), dir);
  assert.equal(payload(dir), "first");
  assert.equal(readFileSync(join(dir, ".installed"), "utf8"), "marker-a");
  assert.deepEqual(readdirSync(root), ["tool"]);
  assert.equal(installNodeTool(dir, "marker-a", populate), dir);
  assert.equal(populated, 1);
});

test("a stale marker is replaced and refresh reinstalls a valid directory", (t) => {
  const root = withRoot(t);
  const dir = join(root, "tool");
  installNodeTool(dir, "marker-a", (staging) => writeFileSync(join(staging, "payload.txt"), "first"));
  installNodeTool(dir, "marker-b", (staging) => writeFileSync(join(staging, "payload.txt"), "second"));
  assert.equal(payload(dir), "second");
  assert.equal(readFileSync(join(dir, ".installed"), "utf8"), "marker-b");
  installNodeTool(dir, "marker-b", (staging) => writeFileSync(join(staging, "payload.txt"), "third"), { refresh: true });
  assert.equal(payload(dir), "third");
  assert.deepEqual(readdirSync(root), ["tool"]);
});

test("a failed populate leaves neither the tool nor its staging directory", (t) => {
  const root = withRoot(t);
  const dir = join(root, "tool");
  assert.throws(() => installNodeTool(dir, "marker-a", () => { throw new Error("npm failed"); }), /npm failed/);
  assert.deepEqual(readdirSync(root), []);
});

test("an install completed by another process during populate wins", (t) => {
  const root = withRoot(t);
  const dir = join(root, "tool");
  const result = installNodeTool(dir, "marker-a", (staging) => {
    writeFileSync(join(staging, "payload.txt"), "loser");
    // Simulate a concurrent installer finishing while this one is still running.
    mkdirSync(dir);
    writeFileSync(join(dir, "payload.txt"), "winner");
    writeFileSync(join(dir, ".installed"), "marker-a");
  });
  assert.equal(result, dir);
  assert.equal(payload(dir), "winner");
  assert.deepEqual(readdirSync(root), ["tool"]);
});

test("a stale directory replaced by another process during populate is not deleted", (t) => {
  const root = withRoot(t);
  const dir = join(root, "tool");
  installNodeTool(dir, "marker-old", (staging) => writeFileSync(join(staging, "payload.txt"), "old"));
  const result = installNodeTool(dir, "marker-new", (staging) => {
    writeFileSync(join(staging, "payload.txt"), "loser");
    rmSync(dir, { recursive: true });
    mkdirSync(dir);
    writeFileSync(join(dir, "payload.txt"), "winner");
    writeFileSync(join(dir, ".installed"), "marker-new");
  });
  assert.equal(result, dir);
  assert.equal(payload(dir), "winner");
  assert.deepEqual(readdirSync(root), ["tool"]);
});

// Each child installs the same tool. Like npm, the fake install resolves for
// a while, writes its package, then keeps working before it returns, and the
// children start staggered so a later one checks the marker during an earlier
// install's final phase. After returning, each child keeps checking that its
// install is still present. With a remove-then-install scheme the later child
// deletes the package the earlier child is about to use and rewrites it only
// after its own resolve phase.
const childSource = `
import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { installNodeTool } from ${JSON.stringify(moduleUrl)};
const [dir, id, startDelay] = process.argv.slice(1);
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
sleep(Number(startDelay));
const result = installNodeTool(dir, "shared-marker", (staging) => {
  sleep(600);
  writeFileSync(join(staging, "payload.txt"), id);
  sleep(300);
});
const deadline = Date.now() + 400;
while (Date.now() < deadline) {
  if (!existsSync(join(result, "payload.txt")) || !existsSync(join(result, ".installed"))) {
    console.error("install vanished for " + id);
    process.exit(1);
  }
  sleep(10);
}
`;

function runChild(dir, id, startDelay) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ["--input-type", "module", "--eval", childSource, "--", dir, id, String(startDelay)], {
      stdio: ["ignore", "ignore", "pipe"],
    });
    let stderr = "";
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("close", (status) => resolve({ status, stderr }));
  });
}

test("concurrent processes installing the same tool all keep a complete install", async (t) => {
  const root = withRoot(t);
  const dir = join(root, "tool");
  const ids = ["a", "b", "c"];
  const results = await Promise.all(ids.map((id, i) => runChild(dir, id, i * 750)));
  for (const result of results) {
    assert.equal(result.status, 0, result.stderr);
  }
  assert.ok(existsSync(join(dir, ".installed")));
  assert.ok(ids.includes(payload(dir)));
  assert.deepEqual(readdirSync(root), ["tool"]);
});

test("worktrees share the main checkout's tool cache", () => {
  const commonDir = () => "/clone/wakaru/.git";
  assert.equal(
    resolveReproToolsRoot("/clone/wakaru-feature", { env: {}, commonDir }),
    join("/clone/wakaru", "target", "repro-tools"),
  );
  assert.equal(
    resolveReproToolsRoot("/clone/wakaru", { env: {}, commonDir }),
    join("/clone/wakaru", "target", "repro-tools"),
  );
});

test("the tool cache stays in the checkout outside a regular clone", () => {
  for (const commonDir of [() => null, () => "/clone/.git/modules/wakaru"]) {
    assert.equal(
      resolveReproToolsRoot("/src/wakaru", { env: {}, commonDir }),
      join("/src/wakaru", "target", "repro-tools"),
    );
  }
});

test("WAKARU_REPRO_TOOLS_DIR overrides the tool cache location", () => {
  const commonDir = () => {
    throw new Error("git must not be consulted when the override is set");
  };
  assert.equal(
    resolveReproToolsRoot("/clone/wakaru", { env: { WAKARU_REPRO_TOOLS_DIR: "cache/tools" }, commonDir }),
    resolve("cache/tools"),
  );
});

test("different markers for one tool name install to different directories", () => {
  const a = nodeToolDir("/tools", "closure-compiler", "lock-a");
  const b = nodeToolDir("/tools", "closure-compiler", "lock-b");
  assert.notEqual(a, b);
  assert.equal(a, nodeToolDir("/tools", "closure-compiler", "lock-a"));
  assert.match(a, /closure-compiler-[0-9a-f]{12}$/);
});
