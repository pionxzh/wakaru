import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { auditTest262Metadata } from "./test262-metadata-audit.mjs";

test("audits every non-fixture Test262 file and reports all metadata errors", () => {
  const root = mkdtempSync(join(tmpdir(), "wakaru-test262-metadata-audit-"));
  try {
    const testDir = join(root, "test", "language", "sample");
    mkdirSync(testDir, { recursive: true });
    writeFileSync(join(testDir, "valid.js"), "/*---\nflags: [noStrict]\n---*/\n");
    writeFileSync(join(testDir, "invalid.js"), "/*---\nflags: [future]\n---*/\n");
    writeFileSync(join(testDir, "dep_FIXTURE.js"), "export const value = 1;\n");

    const result = auditTest262Metadata(root);

    assert.equal(result.discovered, 3);
    assert.equal(result.fixtures, 1);
    assert.equal(result.audited, 2);
    assert.equal(result.valid, 1);
    assert.deepEqual(result.errors, [
      {
        path: "test/language/sample/invalid.js",
        error: "unknown Test262 flag `future`",
      },
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
