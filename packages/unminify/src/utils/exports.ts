import type { ASTNode, Collection, JSCodeshift } from 'jscodeshift'

type Exported = string
type Local = string
type Source = string

export class ExportManager {
    private readonly exports = new Map<Exported, Local>()
    private readonly exportsFrom = new Map<Local, Source>()

    addDefaultExport(local: Local) {
        this.exports.set('default', local)
    }

    addNamedExport(exported: Exported, local: Local) {
        this.exports.set(exported, local)
    }

    addExportFrom(local: Local, source: Source) {
        this.exportsFrom.set(local, source)
    }

    collect(j: JSCodeshift, root: Collection) {
        root
            .find(j.ExportDefaultDeclaration)
            .forEach((path) => {
                const decl = path.node.declaration
                const id = getIdName(j, decl)
                if (id) this.addDefaultExport(id)
            })

        root
            .find(j.ExportNamedDeclaration)
            .forEach((path) => {
                const decl = path.node.declaration
                if (decl) {
                    const id = getIdName(j, decl)
                    if (id) return this.addNamedExport(id, id)

                    if (j.VariableDeclaration.check(decl)) {
                        decl.declarations.forEach((decl) => {
                            if ('id' in decl && j.Identifier.check(decl.id)) {
                                const local = decl.id.name
                                this.addNamedExport(local, local)
                            }
                        })
                    }
                    return
                }

                if (path.node.specifiers) {
                    const source = j.Literal.check(path.node.source) && typeof path.node.source.value === 'string'
                        ? path.node.source.value
                        : null

                    path.node.specifiers.forEach((specifier) => {
                        const exported = j.Identifier.check(specifier.exported)
                            ? specifier.exported.name
                            : typeof specifier.exported === 'string'
                                ? specifier.exported
                                : null
                        if (!exported) return

                        const local = j.Identifier.check(specifier.local)
                            ? specifier.local.name
                            : typeof specifier.local === 'string'
                                ? specifier.local
                                : exported

                        this.addNamedExport(exported, local)
                        if (source) this.addExportFrom(local, source)
                    })
                }
            })
    }
}

function getIdName(j: JSCodeshift, node: ASTNode): string | null {
    if (j.Identifier.check(node)) return node.name
    if (j.FunctionDeclaration.check(node) && j.Identifier.check(node.id)) return node.id.name
    if (j.ClassDeclaration.check(node) && j.Identifier.check(node.id)) return node.id.name
    return null
}
