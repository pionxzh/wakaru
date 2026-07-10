import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

import {
  assertPinnedTest262Corpus,
  inspectTest262Corpus,
  setupTest262Corpus,
  updateTest262Pin,
} from "./test262-corpus.mjs";

test("setup creates a shallow checkout at the configured revision", () => {
  const fixture = makeFixture();
  try {
    const status = setupTest262Corpus(fixture.options());

    assert.equal(status.ready, true);
    assert.equal(status.head, fixture.firstRevision);
    assert.equal(assertPinnedTest262Corpus(fixture.options()).ready, true);
  } finally {
    fixture.cleanup();
  }
});

test("an empty directory inside another repository is not mistaken for its checkout", () => {
  const fixture = makeFixture();
  try {
    const nested = join(fixture.remote, "target", "vendor");
    mkdirSync(nested, { recursive: true });
    const status = inspectTest262Corpus({ root: nested, configPath: fixture.configPath });

    assert.equal(status.repository, false);
  } finally {
    fixture.cleanup();
  }
});

test("setup refuses dirty state unless force is explicit", () => {
  const fixture = makeFixture();
  try {
    setupTest262Corpus(fixture.options());
    writeFileSync(join(fixture.checkout, "dirty.txt"), "local change\n");

    assert.throws(() => setupTest262Corpus(fixture.options()), /refusing to modify dirty/);
    const repaired = setupTest262Corpus({ ...fixture.options(), force: true });
    assert.equal(repaired.ready, true);
  } finally {
    fixture.cleanup();
  }
});

test("offline setup requires the configured commit to exist locally", () => {
  const fixture = makeFixture();
  try {
    setupTest262Corpus(fixture.options());
    const secondRevision = fixture.commit("second.txt", "second\n", "second");
    fixture.writeConfig(secondRevision);

    assert.throws(
      () => setupTest262Corpus({ ...fixture.options(), offline: true }),
      /unavailable offline/,
    );
    assert.equal(setupTest262Corpus(fixture.options()).head, secondRevision);
  } finally {
    fixture.cleanup();
  }
});

test("update verifies the requested revision before rewriting the pin", () => {
  const fixture = makeFixture();
  try {
    setupTest262Corpus(fixture.options());
    const secondRevision = fixture.commit("second.txt", "second\n", "second");

    const status = updateTest262Pin({
      ...fixture.options(),
      revision: secondRevision,
    });

    assert.equal(status.head, secondRevision);
    assert.equal(JSON.parse(readFileSync(fixture.configPath, "utf8")).test262.revision, secondRevision);
    assert.equal(inspectTest262Corpus(fixture.options()).ready, true);
  } finally {
    fixture.cleanup();
  }
});

function makeFixture() {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-corpus-unit-"));
  const remote = join(root, "remote");
  const checkout = join(root, "checkout");
  const configPath = join(root, "upstreams.json");
  mkdirSync(remote, { recursive: true });
  git(remote, ["init"]);
  git(remote, ["config", "user.email", "test@example.com"]);
  git(remote, ["config", "user.name", "Test"]);
  mkdirSync(join(remote, "harness"), { recursive: true });
  writeFileSync(join(remote, "harness", "assert.js"), "void 0;\n");
  git(remote, ["add", "."]);
  git(remote, ["commit", "-m", "initial"]);
  const firstRevision = git(remote, ["rev-parse", "HEAD"]).stdout.trim();

  const writeConfig = (revision) => {
    writeFileSync(
      configPath,
      `${JSON.stringify(
        {
          schemaVersion: 1,
          test262: { url: remote, revision },
        },
        null,
        2,
      )}\n`,
    );
  };
  writeConfig(firstRevision);

  return {
    root,
    remote,
    checkout,
    configPath,
    firstRevision,
    options: () => ({ root: checkout, configPath }),
    writeConfig,
    commit(path, source, message) {
      writeFileSync(join(remote, path), source);
      git(remote, ["add", path]);
      git(remote, ["commit", "-m", message]);
      return git(remote, ["rev-parse", "HEAD"]).stdout.trim();
    },
    cleanup() {
      rmSync(root, { recursive: true, force: true });
    },
  };
}

function git(root, args) {
  const result = spawnSync("git", ["-C", root, ...args], {
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(result.stderr || result.stdout);
  }
  return result;
}
