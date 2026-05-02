const path = require('path');
module.exports = {
  entry: './src/mixed-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-mixed'),
    filename: 'bundle.js',
  },
  mode: 'development',
  devtool: false,
};
