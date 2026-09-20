const path = require("path");
module.exports = {
  entry: "./src/variable-factory-entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp5-variable-factory-min"),
    filename: "bundle.js",
    library: { type: "commonjs2" },
  },
  mode: "production",
  devtool: false,
  target: "web",
  optimization: { concatenateModules: false },
};
