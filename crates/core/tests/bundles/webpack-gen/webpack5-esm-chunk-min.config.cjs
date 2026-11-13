const path = require('path');
module.exports = {
  entry: './src/dynamic-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-esm-chunk-min'),
    filename: 'bundle.js',
    module: true,
    chunkFormat: 'module',
  },
  experiments: { outputModule: true },
  mode: 'production',
  devtool: false,
  target: 'web',
};
