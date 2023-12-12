/* eslint-disable no-console */
import path from 'node:path'
import { Worker } from 'node:worker_threads'
import fsa from 'fs-extra'
import { Timing } from './perf'
import type { UnminifyWorkerParams } from './types'
import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'

export interface UnminifyItem {
    elapsed: number
}

export async function unminify(
    filePath: string,
    moduleMapping: ModuleMapping,
    moduleMeta: ModuleMeta,
    baseDir: string,
    outputDir: string,
) {
    await fsa.ensureDir(outputDir)

    const timing = new Timing()

    const { time: elapsed } = await timing.measureTimeAsync(async () => {
        return runUnminifyInWorker({
            inputPath: filePath,
            outputPath: path.join(outputDir, path.relative(baseDir, filePath)),
            moduleMeta,
            moduleMapping,
        })
    })

    return {
        elapsed,
    }
}

function runUnminifyInWorker(params: UnminifyWorkerParams) {
    return new Promise<string>((resolve, reject) => {
        const worker = new Worker(
            new URL('./unminify.worker.cjs', import.meta.url),
            { workerData: params },
        )

        worker.on('message', resolve)
        worker.on('error', reject)
        worker.on('exit', (code) => {
            if (code !== 0) {
                reject(new Error(`Worker stopped with exit code ${code}`))
            }
        })
    })
}
