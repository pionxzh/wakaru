const path = require("path");

// The same as webpack5-concat-cw-table-min.config.cjs, for webpack 5.110,
// which wraps each body in a function expression instead of an arrow.
module.exports = {
  entry: "./src/concat-cw-table/entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp5-concat-cw-table-fn-min"),
    filename: "bundle.js",
    publicPath: "",
  },
  mode: "production",
  devtool: false,
  target: "web",
  optimization: {
    concatenateModules: true,
  },
};
