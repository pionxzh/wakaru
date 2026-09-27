import assert from "node:assert/strict";
import test from "node:test";
import { parseExactSpec, releaseDateCutoff, resolutionCutoff } from "./release-date.mjs";

test("exact package specs parse; ranges and tags do not", () => {
  assert.deepEqual(parseExactSpec("@babel/core@7.12.17"), { name: "@babel/core", version: "7.12.17" });
  assert.deepEqual(parseExactSpec("@babel/core@8.0.0-rc.5"), { name: "@babel/core", version: "8.0.0-rc.5" });
  assert.deepEqual(parseExactSpec("typescript@5.9.3"), { name: "typescript", version: "5.9.3" });
  assert.equal(parseExactSpec("@swc/core@1"), null);
  assert.equal(parseExactSpec("terser@^5.31.0"), null);
  assert.equal(parseExactSpec("rollup@latest"), null);
  assert.equal(parseExactSpec("typescript"), null);
});

test("the resolution cutoff is one day after the newest publish time", () => {
  assert.equal(
    resolutionCutoff(["2021-02-18T15:13:13.075Z", "2021-02-18T15:13:48.386Z"]),
    "2021-02-19T15:13:48.386Z",
  );
});

test("the release-date cutoff looks up each spec's own publish time", () => {
  const times = {
    "@babel/core": { "7.12.17": "2021-02-18T15:13:48.386Z", "7.12.18": "2021-02-18T16:20:00.000Z" },
    "@babel/preset-env": { "7.12.17": "2021-02-18T15:13:13.075Z" },
  };
  const viewTimes = (name) => JSON.stringify(times[name]);
  assert.equal(
    releaseDateCutoff(["@babel/core@7.12.17", "@babel/preset-env@7.12.17"], viewTimes),
    "2021-02-19T15:13:48.386Z",
  );
  assert.throws(() => releaseDateCutoff(["@babel/core@7.99.0"], viewTimes), /no publish time for @babel\/core@7\.99\.0/);
  assert.throws(() => releaseDateCutoff(["terser@5"], viewTimes), /terser@5 is not an exact version/);
});
