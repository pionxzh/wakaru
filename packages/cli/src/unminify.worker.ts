/* eslint-disable no-console */
import { Timing } from '@wakaru/ast-utils/timing'
import { runDefaultTransformation } from '@wakaru/unminify'
import fsa from 'fs-extra'
import { ThreadWorker } from 'poolifier'
import type { UnminifyWorkerParams } from './types'

export async function unminify(data?: UnminifyWorkerParams) {
    if (!data) throw new Error('No data received')

    const timing = new Timing()
    const { inputPath, outputPath, moduleMeta, moduleMapping } = data
    try {
        const source = await fsa.readFile(inputPath, 'utf-8')
        const fileInfo = { path: inputPath, source }

        const { code } = runDefaultTransformation(fileInfo, { moduleMeta, moduleMapping })
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
