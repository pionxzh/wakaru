import type { ImportDeclaration, JSCodeshift } from 'jscodeshift'

export function addImportSpecifier(j: JSCodeshift, node: ImportDeclaration, specifierName: string) {
    const specifiers = node.specifiers || []

    const existingSpecifier = specifiers.find(s => j.ImportSpecifier.check(s) && s.imported.name === specifierName)
    if (existingSpecifier) return node

    specifiers.push(j.importSpecifier(j.identifier(specifierName)))
    node.specifiers = specifiers

    return node
}

export function tryRemoveUnusedImport(j: JSCodeshift, node: ImportDeclaration) {
    const specifiers = node.specifiers || []

    // TODO

    return null
}
