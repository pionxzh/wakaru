import { defineConfig } from 'tsup'

export default defineConfig({
    entry: ['src/cli.ts', 'src/unminify.worker.ts'],
    format: ['cjs'],
    platform: 'node',
    target: 'node18',
    shims: true,
    dts: false,
    splitting: true,
    sourcemap: false,
    clean: true,
    define: {
        'process.env.NODE_DEBUG': 'undefined',
    },
    minify: true,
    noExternal: [
        '@clack/core', // patched
        '@clack/prompts', // patched
    ],
})
