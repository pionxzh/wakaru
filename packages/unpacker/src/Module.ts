import type { ImportInfo } from '@wakaru/ast-utils/imports'
import type { Collection } from 'jscodeshift'

export class Module {
    /** The module's id */
    id: string | number

    /** Whether the module is the entry module */
    isEntry: boolean

    /** A list of import meta */
    import: ImportInfo[] = []

    /** A map of exported name to local identifier */
    export: Record<string, string> = {}

    /**
     * A map of top-level local identifier to a list of tags.
     * A tag represents a special meaning of the identifier.
     * For example, a function can be marked as a runtime
     * function, and be properly transformed by corresponding
     * rules.
     */
    tags: Record<string, string[]> = {}

    /** The module's code */
    code: string = ''

    constructor(id: string | number, root: Collection, isEntry = false) {
        this.id = id
        this.code = root.toSource()
        this.isEntry = isEntry
    }
}
