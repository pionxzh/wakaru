// Split one wakaru-process budget between concurrent matrix processes.
//
// Each matrix bounds its own wakaru children with WAKARU_REPRO_JOBS, so
// running `matrices` processes with `perMatrix` jobs each keeps the whole
// run at or below `budget` wakaru processes. A budget of 1 therefore still
// runs one matrix at a time with one wakaru child.
//
// Matrix-level concurrency comes first: per-process spawn latency, not CPU,
// dominates a matrix run, so several matrices with small pools finish sooner
// than one matrix with a large pool.
export function planMatrixJobs(budget, matrixCount) {
  if (!Number.isSafeInteger(budget) || budget < 1) {
    throw new Error(`matrix job budget must be a positive integer, got ${budget}`);
  }
  const matrices = Math.max(1, Math.min(matrixCount, budget));
  return { matrices, perMatrix: Math.max(1, Math.floor(budget / matrices)) };
}
