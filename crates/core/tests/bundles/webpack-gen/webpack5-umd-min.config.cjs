const path = require('path');

module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-umd-min'),
    filename: 'bundle.js',
    globalObject: 'self',
    library: {
      name: 'WakaruFixture',
      type: 'umd',
    },
  },
  mode: 'production',
  devtool: false,
  target: 'web',
};
