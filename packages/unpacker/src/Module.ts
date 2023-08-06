import type { Collection } from 'jscodeshift'

export class Module {
    id: string | number
    ast: Collection
    isEntry: boolean

    constructor(id: string | number, ast: Collection, isEntry = false) {
        this.id = id
        this.ast = ast
        this.isEntry = isEntry
    }
}
