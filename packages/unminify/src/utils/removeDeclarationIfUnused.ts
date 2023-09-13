import { mergeComments } from './comments'
import type { ASTPath, ImportDeclaration, JSCodeshift, VariableDeclaration } from 'jscodeshift'

export function removeDeclarationIfUnused(j: JSCodeshift, path: ASTPath, id: string) {
    const closestScope = j(path).closestScope().get()
    if (!closestScope) return

    const idsUsedInScope = j(closestScope).find(j.Identifier, { name: id }).filter((idPath) => {
        const pathScope = idPath.scope.lookup(id)
        return pathScope === closestScope.scope
    })
    const idsUsedInPath = j(path).find(j.Identifier, { name: id })
    const idsUsed = idsUsedInScope.length - idsUsedInPath.length
    if (idsUsed === 1) {
        const idUsed = idsUsedInScope.paths()[0]
        if (j.VariableDeclarator.check(idUsed.parent.node) && j.VariableDeclaration.check(idUsed.parent.parent.node)) {
            const variableDeclaration = idUsed.parent.parent.node as VariableDeclaration
            const index = variableDeclaration.declarations.findIndex(declarator => j.VariableDeclarator.check(declarator)
                && j.Identifier.check(declarator.id)
                && declarator.id.name === id,
            )
            if (index > -1) {
                variableDeclaration.declarations.splice(index, 1)
                if (variableDeclaration.declarations.length === 0) {
                    const currentNodeIndex = idUsed.parent.parent.parent.value?.body?.findIndex((child: any) => child === idUsed.parent.parent.value) as any
                    if (currentNodeIndex > -1) {
                        const nextSibling = idUsed.parent.parent.parent.value.body[currentNodeIndex + 1]
                        if (!nextSibling) return

                        mergeComments(nextSibling, idUsed.parent.parent.value.comments)
                    }
                    idUsed.parent.parent.prune()
                }
            }
        }
    }
}

export function removeDefaultImportIfUnused(j: JSCodeshift, path: ASTPath, id: string) {
    const closestScope = j(path).closestScope().get()
    if (!closestScope) return

    const idsUsedInScope = j(closestScope).find(j.Identifier, { name: id }).filter((idPath) => {
        const pathScope = idPath.scope.lookup(id)
        return pathScope === closestScope.scope
    })
    const idsUsedInPath = j(path).find(j.Identifier, { name: id })
    const idsUsed = idsUsedInScope.length - idsUsedInPath.length
    if (idsUsed === 1) {
        const idUsed = idsUsedInScope.paths()[0]
        if (j.ImportDefaultSpecifier.check(idUsed.parent.node) && j.ImportDeclaration.check(idUsed.parent.parent.node)) {
            const importDeclaration = idUsed.parent.parent.node as ImportDeclaration
            if (!importDeclaration.specifiers) return
            const index = importDeclaration.specifiers.findIndex(declarator => j.ImportDefaultSpecifier.check(declarator)
                && j.Identifier.check(declarator.local)
                && declarator.local.name === id,
            )
            if (index > -1) {
                importDeclaration.specifiers.splice(index, 1)
                if (importDeclaration.specifiers.length === 0) {
                    mergeComments(importDeclaration, idUsed.parent.parent.value.comments)
                    idUsed.parent.parent.prune()
                }
            }
        }
    }
}
