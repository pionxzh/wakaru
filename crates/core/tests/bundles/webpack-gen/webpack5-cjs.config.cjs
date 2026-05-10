const path = require('path');
module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-cjs'),
    filename: 'bundle.js',
  },
  mode: 'development',
  devtool: false,
  target: 'node',
};
