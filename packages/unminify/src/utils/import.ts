import { findDeclaration } from './scope'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTPath, Collection, ImportDeclaration, ImportSpecifier, JSCodeshift, VariableDeclarator } from 'jscodeshift'

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

// import specifierName from 'moduleName'
export function findImportWithDefaultSpecifier(j: JSCodeshift, scope: Scope, specifierName: string): ImportDeclaration | null {
    const declaration = findDeclaration(scope, specifierName)
    if (!declaration) return null

    const importDeclaration = j(declaration).closest(j.ImportDeclaration)
    if (importDeclaration.size() === 0) return null

    const node = importDeclaration.get().node as ImportDeclaration
    if (!node.specifiers || node.specifiers.length === 0) return null

    const specifier = node.specifiers.find(s => j.ImportDefaultSpecifier.check(s) && s.local === declaration.node)
    return specifier ? node : null
}

// import { specifierName } from 'moduleName'
export function findImportWithNamedSpecifier(
    j: JSCodeshift,
    scope: Scope,
    specifierName: string,
    source?: string,
): ASTPath<ImportSpecifier> | null {
    const declaration = findDeclaration(scope, specifierName)
    if (!declaration) return null

    const importSpecifier = j(declaration).closest(j.ImportSpecifier)
    if (importSpecifier.size() === 0) return null

    const path = importSpecifier.get() as ASTPath<ImportSpecifier>

    if (source) {
        const importDeclaration = path.parent.node as ImportDeclaration
        if (importDeclaration.source.value !== source) return null
    }

    return path
}
