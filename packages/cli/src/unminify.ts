/* eslint-disable no-console */
import * as path from 'node:path'
import process from 'node:process'
import { runTransformations, transformationRules } from '@wakaru/unminify'
import fsa from 'fs-extra'
import { Timing } from './perf'
import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'
import type { Transform } from 'jscodeshift'

export interface UnminifyItem {
    elapsed: number
}

export async function unminify(
    paths: string[],
    moduleMapping: ModuleMapping,
    moduleMeta: ModuleMeta,
    baseDir: string,
    outputDir: string,
    perf: boolean,
) {
    await fsa.ensureDir(outputDir)

    const cwd = process.cwd()
    const timing = new Timing(perf)

    const result: UnminifyItem[] = []

    for (const p of paths) {
        const outputPath = path.join(outputDir, path.relative(baseDir, p))
        const filename = path.relative(cwd, outputPath)
        const measure = <T>(key: string, fn: () => T) => timing.collect(filename, key, fn)
        const measureAsync = <T>(key: string, fn: () => Promise<T>) => timing.collectAsync(filename, key, fn)

        const params = { moduleMapping, moduleMeta }

        const { time: elapsed } = await timing.measureTimeAsync(async () => {
            const source = await measureAsync('read file', () => fsa.readFile(p, 'utf-8'))

            const transformations = transformationRules.map<Transform>((rule) => {
                const { id, transform } = rule
                return (...args: Parameters<Transform>) => measure(id, () => transform(...args))
            })
            const result = measure('runDefaultTransformation', () => runTransformations({ path: p, source }, transformations, params))

            await measureAsync('write file', () => fsa.writeFile(outputPath, result.code, 'utf-8'))
        })

        result.push({
            elapsed,
        })
    }

    return result
}
