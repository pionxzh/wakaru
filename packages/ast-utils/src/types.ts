import type { ImportInfo } from '@wakaru/shared/imports'

export type ModuleMapping = Record<number | string, string>

export interface ModuleMeta {
    [moduleId: string]: {
        import: ImportInfo[]
        export: Record<string, string>
        tags: Record<string, string[]>
    }
}
