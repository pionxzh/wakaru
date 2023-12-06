import { defineConfig } from 'vitest/config'

export default defineConfig({
    test: {
        globals: true,
        coverage: {
            provider: 'v8',
            reporter: ['text', 'lcov', 'html'],
            exclude: [
                '**/*.config.*',
                '**/*.d.ts',
                'benches/**',
                'scripts/**',
                'examples/**',
                'testcases/**',
                'packages/browserfs/**',
                'packages/ide/**',
                'packages/playground/**',
            ],
        },
    },
})
