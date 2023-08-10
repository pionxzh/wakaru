import path from 'path'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// https://vitejs.dev/config/
export default defineConfig({
    plugins: [vue()],
    optimizeDeps: {
        include: ['acorn', 'astring'],
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
            '@unminify-kit/unminify': path.resolve(__dirname, '../unminify/src/index.ts'),
            '@unminify-kit/unpacker': path.resolve(__dirname, '../unpacker/src/index.ts'),
        },
    },
})
