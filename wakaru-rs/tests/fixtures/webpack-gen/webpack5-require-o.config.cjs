const path = require('path');

module.exports = {
  entry: {
    bundle: './src/require-o-entry.js',
  },
  output: {
    path: path.resolve(__dirname, 'dist/wp5-require-o'),
    filename: '[name].js',
    chunkFilename: '[name].js',
  },
  mode: 'production',
  devtool: false,
  target: 'web',
  optimization: {
    splitChunks: {
      chunks: 'all',
      minSize: 0,
      cacheGroups: {
        shared: {
          test: /require-o-shared/,
          name: 'shared',
          enforce: true,
        },
      },
    },
  },
};
