import type { Collection } from 'jscodeshift'

export class Module {
    id: string
    ast: Collection<any>
    isEntry: boolean

    constructor(id: string, ast: Collection<any>, isEntry = false) {
        this.id = id
        this.ast = ast
        this.isEntry = isEntry
    }
}
