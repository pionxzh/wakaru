/// <reference types="node" />
import fs from 'node:fs'
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

const inlinedPackages = [
    /^@wakaru\//,
]

const excludeInlinedPackages = [
    '@wakaru/test-utils',
]

const readJson = (pkgPath: string) => {
    const path = new URL(pkgPath, import.meta.url)
    if (!fs.existsSync(path)) {
        throw new Error(`File not found: ${path}`)
    }
    return JSON.parse(fs.readFileSync(path) as any as string)
}
const pkg = readJson('./package.json')
const pkgDeps = Object.keys(pkg.dependencies)
const pkgDevDeps = Object.keys(pkg.devDependencies)

pkgDeps.forEach((dep) => {
    if (inlinedPackages.some(re => re.test(dep)) || excludeInlinedPackages.includes(dep)) {
        throw new Error(`Dependency ${dep} should not be listed in package.json dependencies`)
    }
})

pkgDevDeps.forEach((dep) => {
    const inlined = inlinedPackages.some(re => re.test(dep)) && !excludeInlinedPackages.includes(dep)
    if (!inlined) return

    const depPkg = readJson(`./node_modules/${dep}/package.json`)
    const depDeps = Object.keys(depPkg.dependencies ?? {})
    depDeps.forEach((depDep) => {
        if (!pkgDeps.includes(depDep)) {
            throw new Error(`Dependency ${depDep} from ${dep} should be listed in package.json dependencies`)
        }

        if (excludeInlinedPackages.includes(depDep)) {
            throw new Error(`Dependency ${depDep} from ${dep} should not be listed in ${depDep}'s dependencies`)
        }
    })
})

const moduleRegExp = module => new RegExp(`^${module}(\\/\.+)*$`)
const external = Object.keys(pkg.dependencies).concat('prettier').map(moduleRegExp)

let cache: RollupCache

const dtsOutput = new Set<[string, string]>()

const outputDir = fileURLToPath(new URL('dist', import.meta.url))

const outputMatrix = (
    name: string, format: ModuleFormat[]): OutputOptions[] => {
    const baseName = basename(name)
    return format.flatMap(format => ({
        file: resolve(outputDir, `${baseName}.${format === 'es' ? '' : 'c'}js`),
        sourcemap: true,
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
        external,
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
                sourceMaps: true,
                minify: true,
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
        external,
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
