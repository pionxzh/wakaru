const path = require('path');

module.exports = {
  entry: './src/require-mutation-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-require-mutation-min'),
    filename: 'bundle.js',
  },
  mode: 'production',
  devtool: false,
  target: 'node',
  optimization: {
    moduleIds: 'natural',
    minimize: true,
    concatenateModules: false,
  },
};
