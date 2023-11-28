/* eslint-disable no-console */
import { runTransformationRules, transformationRulesForCLI } from '@wakaru/unminify'
import fsa from 'fs-extra'
import { ThreadWorker } from 'poolifier'
import type { UnminifyWorkerParams } from './types'
import type { Timing } from '@wakaru/shared/timing'

const ruleIds = transformationRulesForCLI.map(rule => rule.id)

export async function unminify(data?: UnminifyWorkerParams) {
    if (!data) throw new Error('No data received')

    const { inputPath, outputPath, moduleMeta, moduleMapping } = data
    try {
        const source = await fsa.readFile(inputPath, 'utf-8')
        const fileInfo = { path: inputPath, source }

        const { code, timing } = await runTransformationRules(fileInfo, ruleIds, { moduleMeta, moduleMapping })
        await fsa.ensureFile(outputPath)
        await fsa.writeFile(outputPath, code, 'utf-8')

        return timing
    }
    catch (e) {
        // We print the error here because it will lose the stack trace after being sent to the main thread
        console.log()
        console.error(e)

        return null
    }
}

export default new ThreadWorker<UnminifyWorkerParams, Timing | null>(unminify)
