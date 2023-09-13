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

export function replaceWithMultipleStatements(
    j: JSCodeshift,
    path: ASTPath<StatementKind>,
    replacements: StatementKind[],
): void {
    const source = j(path).toSource()
    try {
        const currentPath: ASTPath<StatementKind> | null = path
        const parentNode = currentPath.parent?.node

        // If we encounter a Program, directly inject replacements.
        if (j.Program.check(parentNode)) {
            j(currentPath).replaceWith(replacements)
            return
        }

        // If we encounter a BlockStatement, directly inject replacements.
        if (j.BlockStatement.check(parentNode)) {
            // Here you can either replace the current path with your replacements or add them before/after.
            // This code assumes you want to replace the current node with the new nodes.
            j(currentPath).replaceWith(replacements)
            return
        }

        // if (j.ExpressionStatement.check(parentNode)) {
        //     j(currentPath).replaceWith(replacements)
        //     // parentNode.expression = false
        //     return
        // }

        // Handle IfStatement without block
        if (j.IfStatement.check(parentNode)) {
            if (parentNode.consequent === currentPath.node) {
                parentNode.consequent = j.blockStatement(replacements)
                return
            }
            else if (parentNode.alternate === currentPath.node) {
                parentNode.alternate = j.blockStatement(replacements)
                return
            }
        }

        // Handle ArrowFunctionExpression with a single expression body.
        if (j.ArrowFunctionExpression.check(parentNode) && parentNode.expression) {
            parentNode.body = j.blockStatement(replacements)
            parentNode.expression = false
            return
        }

        // Handle SwitchCase
        if (j.SwitchCase.check(parentNode)) {
            const index = parentNode.consequent.indexOf(currentPath.node)
            if (index >= 0) {
                parentNode.consequent.splice(index, 1, ...replacements)
                return
            }
        }

        // Handle loop conditions (while, do-while, for)
        if (
            // @ts-expect-error
            (j.WhileStatement.check(parentNode) && parentNode.test === currentPath.node)
            // @ts-expect-error
            || (j.DoWhileStatement.check(parentNode) && parentNode.test === currentPath.node)
            // @ts-expect-error
            || (j.ForStatement.check(parentNode) && (parentNode.init === currentPath.node || parentNode.update === currentPath.node))
        ) {
            // const blockified = j.callExpression(j.arrowFunctionExpression([], j.blockStatement(replacements)), [])
            // if (j.WhileStatement.check(parentNode) || j.DoWhileStatement.check(parentNode)) {
            //     parentNode.test = blockified
            // }
            // else if (j.ForStatement.check(parentNode) && parentNode.init === currentPath.node) {
            //     parentNode.init = blockified
            // }
            // // @ts-expect-error
            // else if (j.ForStatement.check(parentNode) && parentNode.update === currentPath.node) {
            //     parentNode.update = blockified
            // }

            return
        }

        // Handle loop body
        if ((j.ForStatement.check(parentNode) && parentNode.body === currentPath.node)
         || (j.ForInStatement.check(parentNode) && parentNode.body === currentPath.node)
         || (j.ForOfStatement.check(parentNode) && parentNode.body === currentPath.node)
         || (j.WhileStatement.check(parentNode) && parentNode.body === currentPath.node)
         || (j.DoWhileStatement.check(parentNode) && parentNode.body === currentPath.node)
        ) {
            parentNode.body = j.blockStatement(replacements)
            return
        }

        if (j.LabeledStatement.check(parentNode) && parentNode.body === currentPath.node) {
            // parentNode.body = j.blockStatement(replacements)
            // eslint-disable-next-line no-useless-return
            return // adding a block statement here would be problematic
        }

        // ... potentially handle more cases ...
    }
    catch (e) {
        console.error(e)
        console.error('Source:', source)

        replacements.forEach(r => console.error(j(r).toSource()))
    }
}
