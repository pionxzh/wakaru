// Parameterized rollup config for the synthetic-fixture harness.
// Driven by env vars: H_ENTRY, H_OUT, H_MODE (production|development), H_MIN (1|0)
// Env: H_ENTRY, H_MODE (production|development), H_MIN (1|0), and either
// H_OUT (single-file iife) or H_OUTDIR (code-split esm chunks).
import { nodeResolve } from "@rollup/plugin-node-resolve";
import commonjs from "@rollup/plugin-commonjs";
import replace from "@rollup/plugin-replace";
import terser from "@rollup/plugin-terser";
import json from "@rollup/plugin-json";

const minify = process.env.H_MIN === "1";

export default {
  input: process.env.H_ENTRY,
  output: process.env.H_OUTDIR
    ? { dir: process.env.H_OUTDIR, format: "es", sourcemap: true }
    : { file: process.env.H_OUT, format: "iife", name: "app", sourcemap: true },
  plugins: [
    replace({
      preventAssignment: true,
      "process.env.NODE_ENV": JSON.stringify(process.env.H_MODE || "production"),
    }),
    nodeResolve({ browser: true, preferBuiltins: false }),
    commonjs(),
    json(),
    ...(minify ? [terser()] : []),
  ],
  onwarn() {},
};
