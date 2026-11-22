const path = require("path");

// Wrapped ES and CommonJS modules beside a module table: a lazy chunk keeps
// two of the modules they import in the table.
module.exports = {
  entry: "./src/concat-cw-table/entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp5-concat-cw-table"),
    filename: "bundle.js",
    publicPath: "",
  },
  mode: "production",
  devtool: false,
  target: "web",
  optimization: {
    concatenateModules: true,
    minimize: false,
  },
};
