import path from 'node:path'
import { unpack } from '@wakaru/unpacker'
import fsa from 'fs-extra'
import type { ModuleMapping } from '@wakaru/ast-utils/types'
import type { Module } from '@wakaru/unpacker'

export interface UnpackerItem {
    files: string[]
    modules: Module[]
    moduleIdMapping: ModuleMapping
}

export async function unpacker(
    inputPath: string,
    outputDir: string,
): Promise<UnpackerItem> {
    const files: string[] = []

    const source = await fsa.readFile(inputPath, 'utf-8')
    const { modules, moduleIdMapping } = unpack(source)

    fsa.ensureDirSync(outputDir)
    for (const mod of modules) {
        const filename = moduleIdMapping[mod.id] ?? `module-${mod.id}.js`
        const outputPath = path.join(outputDir, filename)
        await fsa.ensureFile(outputPath)
        await fsa.writeFile(outputPath, mod.code, 'utf-8')
        files.push(outputPath)
    }

    return {
        files,
        modules,
        moduleIdMapping,
    }
}
