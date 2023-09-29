import { scanModule } from './module-scan'
import type { ImportInfo } from '@unminify-kit/ast-utils'
import type { Collection, JSCodeshift } from 'jscodeshift'

export class Module {
    /** The module's id */
    id: string | number

    /** The module's ast from jscodeshift */
    ast: Collection

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
    get code() {
        return this.ast.toSource()
    }

    constructor(id: string | number, j: JSCodeshift, root: Collection, isEntry = false) {
        this.id = id
        this.ast = root
        this.isEntry = isEntry

        scanModule(j, this)
    }
}
