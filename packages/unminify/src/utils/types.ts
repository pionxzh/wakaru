import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'

export type MaybeArray<T> = T | T[]

export interface SharedParams {
    moduleMapping?: ModuleMapping
    moduleMeta?: ModuleMeta
}
