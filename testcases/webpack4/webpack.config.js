module.exports = () => {
    return {
        devtool: false,
        output: {
            filename: 'index.js',
        },
        resolve: {
            extensions: ['.ts', '.tsx', '.js'],
        },
        module: {
            rules: [
                {
                    test: /\.tsx?$/,
                    use: ['babel-loader'],
                },
            ],
        },
    }
}
