module.exports = () => {
    return {
        devtool: false,
        resolve: {
            extensions: [".ts", ".tsx", ".js"],
        },
        module: {
            rules: [
                {
                    test: /\.tsx?$/,
                    use: ["babel-loader"]
                }
            ]
        }
    };
};
