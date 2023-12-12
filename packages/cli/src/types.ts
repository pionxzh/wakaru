import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'

export interface UnminifyWorkerParams {
    inputPath: string
    outputPath: string
    moduleMapping: ModuleMapping
    moduleMeta: ModuleMeta
}
