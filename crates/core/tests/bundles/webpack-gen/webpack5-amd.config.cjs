const path = require('path');

module.exports = {
  entry: './src/index.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-amd'),
    filename: 'bundle.js',
    library: {
      name: 'WakaruFixture',
      type: 'amd',
    },
  },
  mode: 'development',
  devtool: false,
  target: 'web',
};
