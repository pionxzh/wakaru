const path = require('path');
module.exports = {
  entry: './src/esm-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-esm-min'),
    filename: 'bundle.js',
  },
  mode: 'production',
  devtool: false,
};
