const path = require("path");

module.exports = {
  entry: "./src/exports-bridge-entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp5-exports-bridge-min"),
    filename: "bundle.js",
  },
  mode: "production",
  devtool: false,
  target: "web",
  optimization: {
    concatenateModules: false,
  },
};
