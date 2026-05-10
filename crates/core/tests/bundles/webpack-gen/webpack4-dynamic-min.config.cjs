const path = require('path');
module.exports = {
  entry: './src/dynamic-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-dynamic-min'),
    filename: 'bundle.js',
  },
  mode: 'production',
  devtool: false,
};
