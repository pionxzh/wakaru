import path from 'node:path'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// https://vitejs.dev/config/
export default defineConfig({
    plugins: [react()],
    optimizeDeps: {
        esbuildOptions: {
            define: {
                global: 'globalThis',
            },
        },
    },
    define: {
        'process.env.NODE_DEBUG': undefined,
    },
    resolve: {
        alias: {
            '@wakaru/unminify': path.resolve(__dirname, '../unminify/src/index.ts'),
            '@wakaru/unpacker': path.resolve(__dirname, '../unpacker/src/index.ts'),
            // for monaco-editor-auto-typings
            // https://github.com/lukasbach/monaco-editor-auto-typings/issues/25
            'path': 'rollup-plugin-node-polyfills/polyfills/path',
            'node:path': 'rollup-plugin-node-polyfills/polyfills/path',
        },
    },
    server: {
        headers: {
            'Cross-Origin-Embedder-Policy': 'require-corp',
            'Cross-Origin-Opener-Policy': 'same-origin',
        },
    },
})
