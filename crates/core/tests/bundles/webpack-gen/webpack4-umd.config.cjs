const path = require('path');

module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-umd'),
    filename: 'bundle.js',
    globalObject: 'self',
    library: 'WakaruFixture',
    libraryTarget: 'umd',
  },
  mode: 'development',
  devtool: false,
};
