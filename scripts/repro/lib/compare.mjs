// Structural comparison helpers for the reproduction matrices.
//
// These delegate canonicalization to wakaru's own `debug normalize` command,
// which parses, resolves scopes, and (with --rename) alpha-renames every local
// binding to a deterministic name while leaving globals untouched. Two programs
// that differ only by binding names and formatting therefore normalize to byte-
// identical source — so a minifier-mangled recovery can be compared to the
// original snippet without any regex name-stripping.
//
// This replaces the per-matrix regex normalizer: it is scope-correct (no
// property-key / ternary false positives) and non-lossy (distinct bindings keep
// distinct canonical names, so `load_meta(x)` and `load_backup(x)` never
// collapse to the same shape).

import { runPool, runWakaruArgs, runWakaruArgsAsync } from "./runner.mjs";

const cache = new Map();
const cacheKey = (rename, code) => `${rename ? "R" : "F"}\0${code}`;

function normalizeArgs(rename) {
  const args = ["debug", "normalize"];
  if (rename) args.push("--rename");
  args.push("-");
  return args;
}

/**
 * Canonicalize `code` via `wakaru debug normalize`. Reads from the cache, which
 * may have been filled concurrently by {@link prewarmNormalize}.
 * @param {string} code
 * @param {{ rename?: boolean }} [options] rename: alpha-rename local bindings.
 * @returns {string} canonical source, or "" if normalization fails (e.g. the
 *   recovered output does not parse).
 */
export function normalizeCode(code, options = {}) {
  const rename = options.rename ?? false;
  const key = cacheKey(rename, code);
  const hit = cache.get(key);
  if (hit !== undefined) return hit;

  let result;
  try {
    result = runWakaruArgs(normalizeArgs(rename), { input: code }).trim();
  } catch {
    // Unparseable input/output: return a sentinel that can never match a
    // successful normalization, so callers treat it as "not equivalent".
    result = "";
  }
  cache.set(key, result);
  return result;
}

/**
 * Normalize many sources concurrently and fill the cache, so later synchronous
 * {@link normalizeCode} / {@link matchesAnyForm} calls are cache hits. Skips
 * codes already cached. `null`/`undefined` entries are ignored.
 */
export async function prewarmNormalize(codes, options = {}) {
  const rename = options.rename ?? false;
  const args = normalizeArgs(rename);
  const pending = [...new Set(codes)].filter(
    (code) => code != null && !cache.has(cacheKey(rename, code)),
  );
  await runPool(pending, async (code) => {
    let result;
    try {
      result = (await runWakaruArgsAsync(args, { input: code })).trim();
    } catch {
      result = "";
    }
    cache.set(cacheKey(rename, code), result);
  });
}

/**
 * True when `a` and `b` are structurally equal modulo binding names and
 * formatting (alpha-equivalent).
 */
export function structurallyEqual(a, b, options = {}) {
  const na = normalizeCode(a, { rename: true, ...options });
  if (na === "") return false;
  const nb = normalizeCode(b, { rename: true, ...options });
  return na === nb;
}

/**
 * True when `recovered` is alpha-equivalent to any of the accepted full-program
 * `forms`. Use this for mangle shapes: the original snippet plus any genuinely
 * distinct structural variants wakaru may emit (e.g. for-loop vs for-of).
 */
export function matchesAnyForm(recovered, forms) {
  const target = normalizeCode(recovered, { rename: true });
  if (target === "") return false;
  return forms.some((form) => normalizeCode(form, { rename: true }) === target);
}
