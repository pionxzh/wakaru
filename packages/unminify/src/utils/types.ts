import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils'

export type MaybeArray<T> = T | T[]

export interface SharedParams {
    moduleMapping?: ModuleMapping
    moduleMeta?: ModuleMeta
}
