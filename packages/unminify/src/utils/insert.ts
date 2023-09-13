import type { StatementKind } from 'ast-types/lib/gen/kinds'
import type { ASTPath, JSCodeshift } from 'jscodeshift'

export function insertBefore(j: JSCodeshift, path: ASTPath, node: StatementKind) {
    while (path.parent) {
        if (j.Program.check(path.parent.node) || j.BlockStatement.check(path.parent.node)) {
            const body = path.parent.node.body as StatementKind[]
            const index = body.findIndex(p => p === path.node)
            body.splice(index, 0, node)
            break
        }

        path = path.parent
    }
}

export function insertAfter(j: JSCodeshift, path: ASTPath, node: StatementKind) {
    while (path.parent) {
        if (j.Program.check(path.parent.node) || j.BlockStatement.check(path.parent.node)) {
            const body = path.parent.node.body as StatementKind[]
            const index = body.findIndex(p => p === path.node)
            body.splice(index + 1, 0, node)
            break
        }

        path = path.parent
    }
}
