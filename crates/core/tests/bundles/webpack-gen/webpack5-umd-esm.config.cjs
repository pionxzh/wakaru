const path = require('path');

module.exports = {
  entry: './src/esm-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-umd-esm'),
    filename: 'bundle.js',
    globalObject: 'self',
    library: {
      name: 'WakaruFixture',
      type: 'umd',
    },
  },
  mode: 'development',
  devtool: false,
  target: 'web',
};
