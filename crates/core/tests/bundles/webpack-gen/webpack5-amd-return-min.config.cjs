const path = require("path");
module.exports = {
  entry: "./src/amd-return-entry.js",
  output: {
    path: path.resolve(__dirname, "dist/wp5-amd-return-min"),
    filename: "bundle.js",
    library: { type: "commonjs2" },
  },
  mode: "production",
  devtool: false,
  target: "web",
  optimization: { concatenateModules: false },
};
