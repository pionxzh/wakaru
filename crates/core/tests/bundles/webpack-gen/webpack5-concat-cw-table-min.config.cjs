const path = require("path");

// The same as webpack5-concat-cw-table.config.cjs, minified.
module.exports = {
  entry: "./src/concat-cw-table/entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp5-concat-cw-table-min"),
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
