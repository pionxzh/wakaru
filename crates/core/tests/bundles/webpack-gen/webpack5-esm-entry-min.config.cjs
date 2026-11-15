const path = require('path');
module.exports = {
  entry: './src/dynamic-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-esm-entry-min'),
    filename: '[name].js',
    module: true,
    chunkFormat: 'module',
  },
  experiments: { outputModule: true },
  optimization: { runtimeChunk: 'single' },
  mode: 'production',
  devtool: false,
  target: 'web',
};
