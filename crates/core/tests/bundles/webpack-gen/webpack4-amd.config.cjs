const path = require('path');

module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-amd'),
    filename: 'bundle.js',
    library: 'WakaruFixture',
    libraryTarget: 'amd',
  },
  mode: 'development',
  devtool: false,
};
