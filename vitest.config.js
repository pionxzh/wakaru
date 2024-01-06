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
                'packages/cli/**',
                'packages/ide/**',
                'packages/playground/**',

                /**
                 * Specific files to exclude
                 */
                'packages/shared/src/imports.ts',
                'packages/shared/src/timing.ts',
                'packages/shared/src/types.ts',
                'packages/unminify/src/transformations/prettier.ts',
                'packages/unminify/src/transformations/prettier.ts',
            ],
        },
    },
})
