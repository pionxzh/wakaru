import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'
import type { Collection, JSCodeshift } from 'jscodeshift'

/**
 * Find the declaration and wrap it with `export` keyword
 */
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
    const declarationNode = declarationPath?.value
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
        console.warn(j(declarationPath).toSource())
        return
    }

    if (j.VariableDeclaration.check(declarationNode) && declarationNode.declarations.length > 1) {
        // special case for multiple variable declarators
        // e.g. `var a = 1, b = 2`
        const declarators = declarationNode.declarations
        const exportDeclarator = declarators.find((declarator) => {
            return j.VariableDeclarator.check(declarator) && j.Identifier.check(declarator.id) && declarator.id.name === declarationName
        })
        if (!exportDeclarator) {
            console.warn(`Failed to locate export variable declarator: ${declarationName}`)
            return
        }
        const newVariableDeclaration = j.variableDeclaration(declarationNode.kind, [exportDeclarator])
        const newDeclaration = exportName === 'default'
            ? j.exportDefaultDeclaration(newVariableDeclaration)
            : j.exportNamedDeclaration(newVariableDeclaration)
        const filteredDeclaration = j.variableDeclaration(
            declarationNode.kind,
            declarators.filter(declarator => declarator !== exportDeclarator),
        )
        j(declarationPath).replaceWith(filteredDeclaration).insertBefore(newDeclaration)
    }
    else {
        const exportDeclaration = exportName === 'default'
            ? j.exportDefaultDeclaration(declarationNode)
            : j.exportNamedDeclaration(declarationNode)

        j(declarationPath).replaceWith(exportDeclaration)
    }
}

export function prettierFormat(code: string) {
    return prettier.format(code, {
        parser: 'babel',
        plugins: [babelParser],
    })
}
