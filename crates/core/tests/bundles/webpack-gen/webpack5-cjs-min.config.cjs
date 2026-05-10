const path = require('path');
module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-cjs-min'),
    filename: 'bundle.js',
  },
  mode: 'production',
  devtool: false,
  target: 'node',
};
