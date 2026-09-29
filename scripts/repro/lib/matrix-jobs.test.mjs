import assert from "node:assert/strict";
import test from "node:test";

import { planMatrixJobs } from "./matrix-jobs.mjs";

test("a budget of one runs one matrix with one wakaru child", () => {
  assert.deepEqual(planMatrixJobs(1, 13), { matrices: 1, perMatrix: 1 });
});

test("matrix concurrency takes the budget before per-matrix pools", () => {
  assert.deepEqual(planMatrixJobs(10, 13), { matrices: 10, perMatrix: 1 });
  assert.deepEqual(planMatrixJobs(4, 13), { matrices: 4, perMatrix: 1 });
});

test("budget left after one process per matrix goes to the pools", () => {
  assert.deepEqual(planMatrixJobs(16, 13), { matrices: 13, perMatrix: 1 });
  assert.deepEqual(planMatrixJobs(26, 13), { matrices: 13, perMatrix: 2 });
  assert.deepEqual(planMatrixJobs(10, 3), { matrices: 3, perMatrix: 3 });
});

test("the plan never exceeds the budget", () => {
  for (let budget = 1; budget <= 40; budget++) {
    for (let count = 1; count <= 20; count++) {
      const { matrices, perMatrix } = planMatrixJobs(budget, count);
      assert.ok(matrices * perMatrix <= budget, `budget ${budget}, ${count} matrices`);
      assert.ok(matrices <= count);
    }
  }
});

test("rejects a non-positive budget", () => {
  assert.throws(() => planMatrixJobs(0, 13), /positive integer/);
  assert.throws(() => planMatrixJobs(1.5, 13), /positive integer/);
});
