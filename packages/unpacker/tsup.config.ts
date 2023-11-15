import { defineConfig } from 'tsup'

export default defineConfig({
    entry: ['src/index.ts'],
    format: ['cjs', 'esm'],
    dts: true,
    splitting: true,
    sourcemap: true,
    clean: true,
    // minify: true,
    noExternal: [
        // 'jscodeshift',
        // 'ast-types',
        /@wakaru\/.+/,
    ],
})
