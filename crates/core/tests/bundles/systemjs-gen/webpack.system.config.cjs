const path = require("path");

module.exports = {
  mode: "development",
  devtool: false,
  entry: "./webpack-src/entry.js",
  output: {
    path: path.resolve(__dirname, "dist/webpack-system"),
    filename: "bundle.js",
    library: {
      name: "WebpackSystemFixture",
      type: "system",
    },
  },
  optimization: {
    minimize: false,
  },
};
