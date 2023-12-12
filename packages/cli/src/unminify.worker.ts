import path from 'node:path'
import process from 'node:process'
import { parentPort, workerData } from 'node:worker_threads'
import { runTransformations, transformationRules } from '@wakaru/unminify'
import fsa from 'fs-extra'
import { Timing } from './perf'
import type { UnminifyWorkerParams } from './types'
import type { Transform } from 'jscodeshift'

try {
    const { inputPath, outputPath, moduleMeta, moduleMapping } = workerData as UnminifyWorkerParams

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
    fsa.writeFileSync(outputPath, code, 'utf-8')

    parentPort?.postMessage(code)
}
catch (e) {
    // We print the error here because it will lose the stack trace after being sent to the main thread
    console.error(e)
    throw e
}
