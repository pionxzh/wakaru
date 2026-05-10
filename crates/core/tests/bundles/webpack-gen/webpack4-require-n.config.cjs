const path = require('path');
module.exports = {
  entry: './src/require-n-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-require-n'),
    filename: 'bundle.js',
  },
  mode: 'development',
  devtool: false,
};
