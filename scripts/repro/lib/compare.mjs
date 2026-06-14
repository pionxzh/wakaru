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

import { runWakaruArgs } from "./runner.mjs";

const cache = new Map();

/**
 * Canonicalize `code` via `wakaru debug normalize`.
 * @param {string} code
 * @param {{ rename?: boolean }} [options] rename: alpha-rename local bindings.
 * @returns {string} canonical source, or "" if normalization fails (e.g. the
 *   recovered output does not parse).
 */
export function normalizeCode(code, options = {}) {
  const rename = options.rename ?? false;
  const key = `${rename ? "R" : "F"}\0${code}`;
  const hit = cache.get(key);
  if (hit !== undefined) return hit;

  const args = ["debug", "normalize"];
  if (rename) args.push("--rename");
  args.push("-");

  let result;
  try {
    result = runWakaruArgs(args, { input: code }).trim();
  } catch {
    // Unparseable input/output: return a sentinel that can never match a
    // successful normalization, so callers treat it as "not equivalent".
    result = "";
  }
  cache.set(key, result);
  return result;
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
