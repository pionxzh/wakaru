const path = require("path");

module.exports = {
  entry: "./src/concat-cw/pair-entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp5-concat-cw"),
    filename: "bundle.js",
  },
  mode: "production",
  devtool: false,
  target: "node",
  optimization: {
    concatenateModules: true,
    minimize: false,
  },
};
