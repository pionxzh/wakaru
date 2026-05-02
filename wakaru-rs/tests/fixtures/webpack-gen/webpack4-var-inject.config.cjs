const path = require('path');
module.exports = {
  entry: './src/var-inject-entry.js',
  output: {
    path: path.resolve(__dirname, 'dist/wp4-var-inject'),
    filename: 'bundle.js',
  },
  mode: 'development',
  devtool: false,
  target: 'web',
  node: {
    global: true,
    Buffer: true,
  },
};
