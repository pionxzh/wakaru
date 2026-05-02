const path = require('path');
module.exports = {
  entry: './src/dynamic-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-dynamic'),
    filename: 'bundle.js',
  },
  mode: 'development',
  devtool: false,
  target: 'node',
};
