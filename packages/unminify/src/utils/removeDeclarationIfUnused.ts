import type { ASTPath, JSCodeshift, VariableDeclaration } from 'jscodeshift'

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

                        nextSibling.comments = [...new Set([
                            ...(nextSibling.comments || []),
                            ...(idUsed.parent.parent.value.comments || []),
                        ]).values()]
                    }
                    idUsed.parent.parent.prune()
                }
            }
        }
    }
}
