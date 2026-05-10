const path = require('path');
module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-cjs-min'),
    filename: 'bundle.js',
  },
  mode: 'production',
  devtool: false,
};
