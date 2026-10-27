const path = require("path");

module.exports = {
  entry: "./src/module-decorator-entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp4-module-decorator-min"),
    filename: "bundle.js",
  },
  mode: "production",
  devtool: false,
  target: "web",
  optimization: {
    concatenateModules: false,
  },
};
