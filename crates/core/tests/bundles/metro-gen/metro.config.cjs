const path = require("node:path");

module.exports = {
  cacheStores: [],
  maxWorkers: 1,
  projectRoot: __dirname,
  reporter: { update() {} },
  resolver: {
    useWatchman: false,
  },
  server: {
    port: 0,
  },
  transformer: {
    asyncRequireModulePath: require.resolve(
      "metro-runtime/src/modules/asyncRequire",
    ),
    babelTransformerPath: require.resolve("metro-babel-transformer"),
    enableBabelRCLookup: false,
    enableBabelRuntime: false,
    getTransformOptions: async () => ({
      transform: {
        experimentalImportSupport: true,
        inlineRequires: false,
      },
      preloadedModules: false,
      ramGroups: [],
    }),
    globalPrefix: "metro$",
  },
  watchFolders: [path.resolve(__dirname)],
};
