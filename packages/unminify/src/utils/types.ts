import type { ModuleMapping, ModuleMeta } from '@unminify-kit/ast-utils'

export type MaybeArray<T> = T | T[]

export interface SharedParams {
    moduleMapping?: ModuleMapping
    moduleMeta?: ModuleMeta
}
