module.exports = {
  mode: "production",
  entry: "./src/index.js",
  devtool: "source-map",
  output: {
    path: __dirname + "/dist",
    filename: "main.[contenthash:8].js",
    chunkFilename: "[name].[contenthash:8].js",
    clean: true,
  },
};
