const path = require('path');
module.exports = {
  entry: './src/dynamic-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/rspack-chunk-es2022-min'),
    filename: 'bundle.js',
  },
  mode: 'production',
  devtool: false,
  target: ['web', 'es2022'],
};
