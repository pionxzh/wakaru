import type { Collection, ImportDeclaration, JSCodeshift, VariableDeclarator } from 'jscodeshift'

/**
 * Find the module source of given module name.
 */
export function findModuleSource(j: JSCodeshift, root: Collection, moduleName: string): ImportDeclaration | VariableDeclarator | null {
    // import mod from 'moduleName'
    const importDeclarations = root.find(j.ImportDeclaration, {
        specifiers: [{ type: 'ImportDefaultSpecifier', local: { type: 'Identifier' } }],
        source: { type: 'Literal', value: moduleName },
    })
    if (importDeclarations.size() > 0) {
        return importDeclarations.get().node
    }

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
