const path = require('path');
module.exports = {
  entry: './src/dynamic-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-dynamic'),
    filename: 'bundle.js',
  },
  mode: 'development',
  devtool: false,
};
