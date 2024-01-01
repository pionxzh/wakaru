/* eslint-disable no-console */
import path from 'node:path'
import process from 'node:process'
import { Timing } from '@wakaru/ast-utils/timing'
import { runTransformations, transformationRules } from '@wakaru/unminify'
import fsa from 'fs-extra'
import { ThreadWorker } from 'poolifier'
import type { UnminifyWorkerParams } from './types'
import type { Transform } from 'jscodeshift'

export async function unminify(data?: UnminifyWorkerParams) {
    if (!data) throw new Error('No data received')

    const timing = new Timing()
    const { inputPath, outputPath, moduleMeta, moduleMapping } = data
    try {
        const cwd = process.cwd()
        const filename = path.relative(cwd, inputPath)
        const source = await fsa.readFile(inputPath, 'utf-8')
        const fileInfo = { path: inputPath, source }

        const transformations = transformationRules.map<Transform>((rule) => {
            const { id, transform } = rule
            const fn = (...args: Parameters<Transform>) => timing.collect(filename, id, () => transform(...args))
            // Set the name of the function for better debugging
            Object.defineProperty(fn, 'name', { value: id })
            return fn
        })

        const { code } = runTransformations(fileInfo, transformations, { moduleMeta, moduleMapping })
        await fsa.ensureFile(outputPath)
        await fsa.writeFile(outputPath, code, 'utf-8')

        return timing
    }
    catch (e) {
        // We print the error here because it will lose the stack trace after being sent to the main thread
        console.log()
        console.error(e)

        return timing
    }
}

export default new ThreadWorker<UnminifyWorkerParams, Timing>(unminify)
