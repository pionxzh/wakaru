import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'
import type { Module } from '@wakaru/unpacker'

export type FileIdList = Array<number | string>

export type TransformedModule = Module & {
    /** The transformed module code */
    transformed: string

    /** Error message if any */
    error?: string
}

export interface CodeModParams {
    name: string
    module: TransformedModule
    transformationRuleIds: string[]
    moduleMeta: ModuleMeta
    moduleMapping: ModuleMapping
}

export interface UnpackerResult {
    modules: Module[]
    moduleIdMapping: ModuleMapping
}
