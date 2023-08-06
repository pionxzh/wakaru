import type { Collection } from 'jscodeshift'

export class Module {
    /** The module id */
    id: string | number
    /** The module ast from jscodeshift */
    ast: Collection
    /** Whether the module is the entry module */
    isEntry: boolean

    /** The module code */
    get code() {
        return this.ast.toSource()
    }

    constructor(id: string | number, ast: Collection, isEntry = false) {
        this.id = id
        this.ast = ast
        this.isEntry = isEntry
    }
}
