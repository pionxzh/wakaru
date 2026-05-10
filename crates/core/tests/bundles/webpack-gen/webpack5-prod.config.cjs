const path = require('path');
module.exports = {
  entry: './src/esm-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-prod'),
    filename: 'bundle.js',
  },
  mode: 'production',
  devtool: false,
  optimization: {
    minimize: false,
  },
};
