const path = require('path');
module.exports = {
  entry: './src/esm-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-esm'),
    filename: 'bundle.js',
  },
  mode: 'development',
  devtool: false,
  target: 'node',
  experiments: {
    outputModule: false,
  },
};
