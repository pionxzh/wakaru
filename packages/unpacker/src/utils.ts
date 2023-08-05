import type { ASTPath, ArrowFunctionExpression, ClassDeclaration, Collection, ExpressionStatement, FunctionDeclaration, FunctionExpression, JSCodeshift, Node, Statement, VariableDeclaration } from 'jscodeshift'
import prettier from 'prettier/standalone'
import babelParser from 'prettier/parser-babel'

export function isTopLevel(j: JSCodeshift, node: ASTPath<Node>): boolean {
    return j.Program.check(node.parentPath.node)
}

export function pruneComments(j: JSCodeshift, collection: Collection<any>): void {
    // @ts-expect-error - Comment type is wrong
    collection.find(j.Comment).forEach(path => path.prune())
}

export function renameFunctionParameters(j: JSCodeshift, node: FunctionExpression | ArrowFunctionExpression, parameters: string[]): void {
    node.params.forEach((param, index) => {
        if (param.type === 'Identifier') {
            const oldName = param.name
            const newName = parameters[index]

            // Only get the immediate function scope
            const functionScope = j(node).closestScope().get()

            // Check if the name is in the current scope and rename it
            if (functionScope.scope.getBindings()[oldName]) {
                j(functionScope)
                    .find(j.Identifier, { name: oldName })
                    .forEach((path) => {
                        // Exclude MemberExpression properties
                        if (!(path.parent.node.type === 'MemberExpression' && path.parent.node.property === path.node)
                            && path.scope.node === functionScope.node) {
                            path.node.name = newName
                        }
                    })
            }
        }
    })
}

export function wrapDeclarationWithExport(
    j: JSCodeshift,
    collection: Collection<any>,
    exportName: string,
    declarationName: string,
): void {
    const globalScope = collection.get().scope

    if (!globalScope.getBindings()[declarationName]) {
        console.warn('Failed to locate export value:', declarationName)
        return
    }

    const declarations = globalScope.getBindings()[declarationName]
    if (declarations.length !== 1) {
        console.warn(`Expected exactly one class declaration for ${declarationName}, found ${declarations.length} instead`)
        return
    }
    const declarationPath = declarations[0].parent?.parent
    const declarationNode = declarationPath.value
    if (!declarationNode) {
        console.warn('Failed to locate declaration node:', declarationName)
        return
    }

    // Skip program nodes
    if (j.Program.check(declarationNode)) return

    if (!j.VariableDeclaration.check(declarationNode)
    && !j.FunctionDeclaration.check(declarationNode)
    && !j.ClassDeclaration.check(declarationNode)) {
        console.warn(`Declaration is not a variable, function or class: ${declarationName}, the type is ${declarationNode.type}`)
        return
    }

    const exportDeclaration = exportName === 'default'
        ? j.exportDefaultDeclaration(declarationNode)
        : j.exportNamedDeclaration(declarationNode)

    j(declarationPath).replaceWith(exportDeclaration)
}

export function isIIFE(node: Statement): node is ExpressionStatement {
    if (node.type !== 'ExpressionStatement') return false
    const expression = (node as ExpressionStatement).expression
    if (expression.type !== 'CallExpression') return false
    const callee = expression.callee
    return callee.type === 'FunctionExpression'
        || callee.type === 'ArrowFunctionExpression'
}

export function prettierFormat(code: string) {
    return prettier.format(code, {
        parser: 'babel',
        plugins: [babelParser],
    })
}
