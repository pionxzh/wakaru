import path from 'node:path'
import { unpack } from '@wakaru/unpacker'
import fsa from 'fs-extra'
import { Timing } from './timing'
import type { ModuleMapping } from '@wakaru/ast-utils/types'
import type { Module } from '@wakaru/unpacker'

export interface UnpackerItem {
    files: string[]
    modules: Module[]
    moduleIdMapping: ModuleMapping
    elapsed: number
}

export async function unpacker(
    paths: string[],
    outputDir: string,
): Promise<UnpackerItem[]> {
    fsa.ensureDirSync(outputDir)

    const result: UnpackerItem[] = []
    const files: string[] = []

    for (const p of paths) {
        const source = await fsa.readFile(p, 'utf-8')

        const timing = new Timing()
        const { result: { modules, moduleIdMapping }, time: elapsed } = timing.measureTime(() => unpack(source))

        for (const mod of modules) {
            const filename = moduleIdMapping[mod.id] ?? `module-${mod.id}.js`
            const outputPath = path.join(outputDir, filename)
            await fsa.ensureFile(outputPath)
            await fsa.writeFile(outputPath, mod.code, 'utf-8')
            files.push(outputPath)
        }

        result.push({
            files,
            modules,
            moduleIdMapping,
            elapsed,
        })
    }
    return result
}
