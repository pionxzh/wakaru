import path from 'node:path'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

// https://vitejs.dev/config/
export default defineConfig({
    plugins: [vue()],
    optimizeDeps: {
        esbuildOptions: {
            define: {
                global: 'globalThis',
            },
        },
    },
    define: {
        'process.env.NODE_DEBUG': undefined,
        'typeof window !== "undefined" && typeof window.document !== "undefined"': true,
    },
    resolve: {
        alias: {
            '@unminify-kit/unminify': path.resolve(__dirname, '../unminify/src/index.ts'),
            '@unminify-kit/unpacker': path.resolve(__dirname, '../unpacker/src/index.ts'),
        },
    },
})
