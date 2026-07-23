const path = require('path');
// Dense natural numeric ids force the array-form modules container
// (Template.getModulesArrayBounds): the main bundle renders a sparse array,
// and the lazy chunk's ids start high enough that webpack wraps its table in
// Array(minId).concat([...]).
module.exports = {
  entry: './src/array/entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp5-array'),
    filename: 'bundle.js',
    chunkFilename: 'chunk-[id].js',
  },
  mode: 'development',
  devtool: false,
  optimization: {
    moduleIds: 'natural',
    chunkIds: 'natural',
    minimize: false,
    concatenateModules: false,
  },
};
