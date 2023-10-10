import { isTopLevel } from './isTopLevel'
import type { ASTNode, AssignmentExpression, Collection, Identifier, JSCodeshift, MemberExpression, VariableDeclarator } from 'jscodeshift'

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

    collectEsModuleExport(j: JSCodeshift, root: Collection) {
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
                    const source = j.StringLiteral.check(path.node.source)
                        ? path.node.source.value
                        : null

                    path.node.specifiers.forEach((specifier) => {
                        const exported = j.Identifier.check(specifier.exported)
                            ? specifier.exported.name
                            : null
                        if (!exported) return

                        const local = j.Identifier.check(specifier.local)
                            ? specifier.local.name
                            : exported

                        this.addNamedExport(exported, local)
                        if (source) this.addExportFrom(local, source)
                    })
                }
            })
    }

    collectCommonJsExport(j: JSCodeshift, root: Collection) {
        /**
         * Default export
         *
         * Note: `exports = { ... }` is not valid
         * So we won't handle it
         *
         * @example
         * module.exports = 1
         * ->
         * export default 1
         *
         * @example
         * module.exports = { foo: 1 }
         * ->
         * export default { foo: 1 }
         */
        root
            .find(j.ExpressionStatement, {
                expression: {
                    type: 'AssignmentExpression',
                    operator: '=',
                    left: {
                        type: 'MemberExpression',
                        object: {
                            type: 'Identifier',
                            name: 'module',
                        },
                        property: {
                            type: 'Identifier',
                            name: 'exports',
                        },
                    },
                },
            })
            .forEach((path) => {
                if (!isTopLevel(j, path)) return

                const expression = path.node.expression as AssignmentExpression
                if (j.Identifier.check(expression.right)) {
                    this.addDefaultExport(expression.right.name)
                }
                else if (j.ObjectExpression.check(expression.right)) {
                    const object = expression.right
                    const properties = object.properties
                    properties.forEach((property) => {
                        if (j.ObjectProperty.check(property)) {
                            const key = property.key
                            const value = property.value
                            if (j.Identifier.check(key) && j.Identifier.check(value)) {
                                this.addNamedExport(key.name, value.name)
                            }
                        }
                    })
                }
            })

        /**
         * Individual exports
         *
         * @example
         * module.exports.foo = 1
         * ->
         * export const foo = 1
         *
         * @example
         * module.exports.foo = foo
         * ->
         * export { foo }
         */
        root
            .find(j.ExpressionStatement, {
                expression: {
                    type: 'AssignmentExpression',
                    operator: '=',
                    left: {
                        type: 'MemberExpression',
                        object: {
                            type: 'MemberExpression',
                            object: {
                                type: 'Identifier',
                                name: 'module',
                            },
                            property: {
                                type: 'Identifier',
                                name: 'exports',
                            },
                        },
                        property: {
                            type: 'Identifier',
                        },
                    },
                },
            })
            .forEach((path) => {
                if (!isTopLevel(j, path)) return

                const expression = path.node.expression as AssignmentExpression
                const left = expression.left as MemberExpression
                const name = (left.property as Identifier).name
                this.addNamedExport(name, name)
            })

        /**
         * Individual exports
         *
         * @example
         * exports.foo = 2
         * ->
         * export const foo = 2
         */
        root
            .find(j.ExpressionStatement, {
                expression: {
                    type: 'AssignmentExpression',
                    operator: '=',
                    left: {
                        type: 'MemberExpression',
                        object: {
                            type: 'Identifier',
                            name: 'exports',
                        },
                        property: {
                            type: 'Identifier',
                        },
                    },
                },
            })
            .forEach((path) => {
                if (!isTopLevel(j, path)) return

                const expression = path.node.expression as AssignmentExpression
                const left = expression.left as MemberExpression
                const name = (left.property as Identifier).name
                this.addNamedExport(name, name)
            })

        /**
         * Special case:
         *
         * Note: This pattern has been dropped by Babel in https://github.com/babel/babel/pull/15984
         *
         * @example
         * var foo = exports.foo = 1
         */
        root
            .find(j.VariableDeclaration, {
                declarations: [
                    {
                        type: 'VariableDeclarator',
                        id: {
                            type: 'Identifier',
                        },
                        init: {
                            type: 'AssignmentExpression',
                            operator: '=',
                            left: {
                                type: 'MemberExpression',
                                object: {
                                    type: 'Identifier',
                                    name: 'exports',
                                },
                                property: {
                                    type: 'Identifier',
                                },
                            },
                        },
                    },
                ],
            })
            .forEach((path) => {
                if (!isTopLevel(j, path)) return

                const declaration = path.node.declarations[0] as VariableDeclarator
                const init = declaration.init as AssignmentExpression
                const left = init.left as MemberExpression
                const right = init.right

                const name = (left.property as Identifier).name

                if (j.Identifier.check(right)) {
                    if (name === 'default') {
                        this.addDefaultExport(right.name)
                    }
                    else {
                        this.addNamedExport(name, right.name)
                    }
                }
            })

        /**
         * Special case:
         *
         * @example
         * var bar = module.exports.baz = 2
         */
        root
            .find(j.VariableDeclaration, {
                declarations: [
                    {
                        type: 'VariableDeclarator',
                        id: {
                            type: 'Identifier',
                        },
                        init: {
                            type: 'AssignmentExpression',
                            operator: '=',
                            left: {
                                type: 'MemberExpression',
                                object: {
                                    type: 'MemberExpression',
                                    object: {
                                        type: 'Identifier',
                                        name: 'module',
                                    },
                                    property: {
                                        type: 'Identifier',
                                        name: 'exports',
                                    },
                                },
                                property: {
                                    type: 'Identifier',
                                },
                            },
                        },
                    },
                ],
            })
            .forEach((path) => {
                if (!isTopLevel(j, path)) return

                const declaration = path.node.declarations[0] as VariableDeclarator
                const init = declaration.init as AssignmentExpression
                const left = init.left as MemberExpression
                const right = init.right

                const name = (left.property as Identifier).name

                if (j.Identifier.check(right)) {
                    if (name === 'default') {
                        this.addDefaultExport(right.name)
                    }
                    else {
                        this.addNamedExport(name, right.name)
                    }
                }
            })
    }

    toJSON(): Record<Exported, Local> {
        return Object.fromEntries(this.exports)
    }
}

function getIdName(j: JSCodeshift, node: ASTNode): string | null {
    if (j.Identifier.check(node)) return node.name
    if (j.FunctionDeclaration.check(node) && j.Identifier.check(node.id)) return node.id.name
    if (j.ClassDeclaration.check(node) && j.Identifier.check(node.id)) return node.id.name
    return null
}
