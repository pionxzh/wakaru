const path = require('path');

module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-umd'),
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
