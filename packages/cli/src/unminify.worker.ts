import path from 'node:path'
import process from 'node:process'
import { runTransformations, transformationRules } from '@wakaru/unminify'
import fsa from 'fs-extra'
import { ThreadWorker } from 'poolifier'
import { Timing } from './timing'
import type { Measurement } from './timing'
import type { UnminifyWorkerParams } from './types'
import type { Transform } from 'jscodeshift'

export async function unminify(data?: UnminifyWorkerParams) {
    if (!data) throw new Error('No data received')

    try {
        const { inputPath, outputPath, moduleMeta, moduleMapping } = data

        const cwd = process.cwd()
        const filename = path.relative(cwd, inputPath)
        const source = await fsa.readFile(inputPath, 'utf-8')
        const fileInfo = { path: inputPath, source }

        const timing = new Timing()
        const transformations = transformationRules.map<Transform>((rule) => {
            const { id, transform } = rule
            return (...args: Parameters<Transform>) => timing.collect(filename, id, () => transform(...args))
        })

        const { code } = runTransformations(fileInfo, transformations, { moduleMeta, moduleMapping })
        await fsa.ensureFile(outputPath)
        await fsa.writeFile(outputPath, code, 'utf-8')

        return timing.getMeasurements()
    }
    catch (e) {
        // We print the error here because it will lose the stack trace after being sent to the main thread
        console.error(e)
        throw e
    }
}

export default new ThreadWorker<UnminifyWorkerParams, Measurement>(unminify)
