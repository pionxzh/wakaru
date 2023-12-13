import path from 'node:path'
import process from 'node:process'
import { runTransformations, transformationRules } from '@wakaru/unminify'
import fsa from 'fs-extra'
import { ThreadWorker } from 'poolifier'
import { Timing } from './perf'
import type { UnminifyWorkerParams } from './types'
import type { Transform } from 'jscodeshift'

export function unminify(data?: UnminifyWorkerParams) {
    if (!data) throw new Error('No data received')

    try {
        const { inputPath, outputPath, moduleMeta, moduleMapping } = data

        const timing = new Timing()
        const cwd = process.cwd()
        const filename = path.relative(cwd, inputPath)
        const measure = <T>(key: string, fn: () => T) => timing.collect(filename, key, fn)

        const source = fsa.readFileSync(inputPath, 'utf-8')
        const fileInfo = { path: inputPath, source }

        const transformations = transformationRules.map<Transform>((rule) => {
            const { id, transform } = rule
            return (...args: Parameters<Transform>) => measure(id, () => transform(...args))
        })

        const { code } = runTransformations(fileInfo, transformations, { moduleMeta, moduleMapping })
        fsa.ensureFileSync(outputPath)
        fsa.writeFileSync(outputPath, code, 'utf-8')

        return code
    }
    catch (e) {
        // We print the error here because it will lose the stack trace after being sent to the main thread
        console.error(e)
        throw e
    }
}

export default new ThreadWorker<UnminifyWorkerParams, string>(unminify)
