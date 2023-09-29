import type { Collection, Statement } from 'jscodeshift'

export function getTopLevelStatements(root: Collection) {
    const body = root.get().node.program.body as Statement[]
    return body
}
