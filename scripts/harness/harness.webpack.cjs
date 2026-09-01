// Parameterized webpack5 config for the synthetic-fixture harness.
// Usage: npx webpack --config harness.webpack.cjs \
//          --env entry=/abs/src/main.js --env outdir=/abs/dist --env mode=production
module.exports = (env) => ({
  entry: env.entry,
  mode: env.mode || "production",
  devtool: "source-map",
  target: "web",
  output: {
    path: env.outdir,
    filename: "bundle.js",
  },
  performance: { hints: false },
  stats: "errors-only",
});
