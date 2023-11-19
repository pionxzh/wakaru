import { defineConfig } from 'tsup'

export default defineConfig({
    entry: ['src/index.ts', 'src/cli.ts'],
    format: ['cjs', 'esm'],
    dts: true,
    splitting: true,
    sourcemap: true,
    clean: true,
    define: {
        'process.env.NODE_DEBUG': 'undefined',
    },
    // minify: true,
    noExternal: [
        'jscodeshift',
        'ast-types',
        /@wakaru\/.+/,
    ],
})
