const path = require("path");

module.exports = {
  entry: "./src/inner-umd-entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp5-inner-umd-min"),
    filename: "bundle.js",
  },
  mode: "production",
  devtool: false,
  target: "web",
  optimization: {
    concatenateModules: false,
  },
};
