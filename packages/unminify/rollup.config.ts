/// <reference types="node" />
import { basename, resolve } from 'node:path'

import { fileURLToPath } from 'node:url'
import commonjs from '@rollup/plugin-commonjs'
import { nodeResolve } from '@rollup/plugin-node-resolve'
import dts from 'rollup-plugin-dts'
import { defineRollupSwcOption, swc } from 'rollup-plugin-swc3'
import type {
    ModuleFormat,
    OutputOptions,
    RollupCache,
    RollupOptions,
} from 'rollup'

let cache: RollupCache

const dtsOutput = new Set<[string, string]>()

const outputDir = fileURLToPath(new URL('dist', import.meta.url))

const outputMatrix = (
    name: string, format: ModuleFormat[]): OutputOptions[] => {
    const baseName = basename(name)
    return format.flatMap(format => ({
        file: resolve(outputDir, `${baseName}.${format === 'es' ? '' : 'c'}js`),
        sourcemap: false,
        format,
        banner: `/// <reference types="./${baseName}.d.ts" />`,
    }))
}

const buildMatrix = (input: string, output: string, config: {
    format: ModuleFormat[]
    dts: boolean
}): RollupOptions => {
    if (config.dts) {
        dtsOutput.add([input, output])
    }
    return {
        input,
        output: outputMatrix(output, config.format),
        cache,
        plugins: [
            commonjs(),
            nodeResolve({
                preferBuiltins: true,
            }),
            swc(defineRollupSwcOption({
                jsc: {
                    externalHelpers: false,
                    parser: {
                        syntax: 'typescript',
                    },
                    target: 'es2020',
                },
                tsconfig: false,
            })),
        ],
    }
}

const dtsMatrix = (): RollupOptions[] => {
    return [...dtsOutput.values()].flatMap(([input, output]) => ({
        input,
        cache,
        output: {
            file: resolve(outputDir, `${output}.d.ts`),
            format: 'es',
        },
        plugins: [
            nodeResolve({
                preferBuiltins: true,
            }),
            dts({
                respectExternal: true,
            }),
        ],
    }))
}

const build: RollupOptions[] = [
    buildMatrix('./src/index.ts', 'index', {
        format: ['es', 'cjs'],
        dts: true,
    }),
    ...dtsMatrix(),
]

export default build
