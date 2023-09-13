import type { Collection, ImportDeclaration, JSCodeshift, VariableDeclarator } from 'jscodeshift'

/**
 * Find the module source of given module name.
 */
export function findModuleFromSource(j: JSCodeshift, root: Collection, moduleName: string): ImportDeclaration | VariableDeclarator | null {
    return findImportFromSource(j, root, moduleName)
    ?? findRequireFromSource(j, root, moduleName)
}

export function findImportFromSource(j: JSCodeshift, root: Collection, moduleName: string): ImportDeclaration | null {
    // import mod from 'moduleName'
    const importDeclarations = root.find(j.ImportDeclaration, {
        source: { type: 'Literal', value: moduleName },
    })
    if (importDeclarations.size() > 0) {
        return importDeclarations.get().node
    }

    return null
}

export function findRequireFromSource(j: JSCodeshift, root: Collection, moduleName: string): VariableDeclarator | null {
    // const mod = require('moduleName')
    const variableDeclarators = root.find(j.VariableDeclarator, {
        init: {
            type: 'CallExpression',
            callee: { type: 'Identifier', name: 'require' },
            arguments: [{ type: 'Literal', value: moduleName } as const],
        },
    })
    if (variableDeclarators.size() > 0) {
        return variableDeclarators.get().node
    }

    return null
}

export function findImportWithDefaultSpecifier(j: JSCodeshift, root: Collection, specifierName: string): ImportDeclaration | null {
    const importDefaultSpecifier = root.find(j.ImportDefaultSpecifier, {
        local: { type: 'Identifier' },
    })
    if (importDefaultSpecifier.size() > 0) {
        const importDeclaration = importDefaultSpecifier.closest(j.ImportDeclaration)
        if (importDeclaration.size() > 0) {
            return importDeclaration.get().node
        }
    }

    return null
}
