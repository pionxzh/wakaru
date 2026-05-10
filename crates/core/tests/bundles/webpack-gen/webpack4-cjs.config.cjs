const path = require('path');
module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-cjs'),
    filename: 'bundle.js',
  },
  mode: 'development',
  devtool: false,
};
