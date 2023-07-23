/** @type { import('webpack').Configuration } */
export default {
    mode: 'development',
    devtool: 'source-map',
    entry: './src/index.js',
    output: {
        filename: 'index.js',
    },
}
