import wrap from '../wrapAstTransformation'
import type { ASTTransformation, Context } from '../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { NodePath } from 'ast-types/lib/node-path'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTPath, AssignmentExpression, CallExpression, Identifier, ImportDeclaration, JSCodeshift, Literal, MemberExpression, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

type ImportInfo = {
    name: string
    source: string
    type: 'default' | 'named' | 'namespace'
} | {
    name: null
    source: string
    type: 'bare'
}

interface Params {
    hoist?: boolean
}

/**
 * Converts cjs require/exports syntax to esm import/export syntax
 *
 * @example
 * var foo = require('foo')
 * var { bar } = require('bar')
 * var baz = require('baz').baz
 * require('side-effect')
 * ->
 * import foo from 'foo'
 * import { bar } from 'bar'
 * import { baz } from 'baz'
 * import 'side-effect'
 *
 * @example
 * module.exports.foo = foo
 * module.exports.bar = bar
 * exports.baz = baz
 * ->
 * export { foo, bar, baz }
 */
export const transformAST: ASTTransformation<Params> = (context, params) => {
    const hoist = params?.hoist ?? false

    transformImport(context, hoist)
    transformExport(context)
}

/**
 * Limitations:
 * - require with variable cannot be transformed
 */
// TODO: hoist
function transformImport(context: Context, hoist: boolean) {
    const { root, j } = context

    /**
     * Steps:
     * 1. Collect requires
     * 2. Reconstruct imports with deduplication
     */

    const imports: ImportInfo[] = []

    /**
     * Collect imports
     */
    root
        .find(j.ImportDeclaration)
        .forEach((path) => {
            const { specifiers, source } = path.node
            if (!j.Literal.check(source) || typeof source.value !== 'string') return

            if (!specifiers) {
                imports.push({
                    name: null,
                    source: source.value,
                    type: 'bare',
                })
                j(path).remove()
                return
            }

            const sourceValue = source.value

            specifiers.forEach((specifier) => {
                if (j.ImportDefaultSpecifier.check(specifier)
                 && j.Identifier.check(specifier.local)) {
                    imports.push({
                        name: specifier.local.name,
                        source: sourceValue,
                        type: 'default',
                    })
                }

                if (j.ImportSpecifier.check(specifier)
                    && j.Identifier.check(specifier.imported)
                    && j.Identifier.check(specifier.local)
                ) {
                    imports.push({
                        name: specifier.local.name,
                        source: sourceValue,
                        type: 'named',
                    })
                }

                if (j.ImportNamespaceSpecifier.check(specifier)
                    && j.Identifier.check(specifier.local)
                ) {
                    imports.push({
                        name: specifier.local.name,
                        source: sourceValue,
                        type: 'namespace',
                    })
                }
            })

            j(path).remove()
        })

    /*
     * Basic require and require with destructuring
     *
     * @example
     * var foo = require('foo')
     * var { bar } = require('bar')
     */
    root
        .find(j.VariableDeclaration, {
            declarations: [
                {
                    type: 'VariableDeclarator',
                    init: {
                        type: 'CallExpression',
                        callee: {
                            type: 'Identifier',
                            name: 'require',
                        },
                        arguments: [{ type: 'Literal' as const }],
                    },
                },
            ],
        })
        .forEach((path) => {
            const firstDeclaration = path.node.declarations[0] as VariableDeclarator
            const id = firstDeclaration.id
            const init = firstDeclaration.init as CallExpression

            const sourceLiteral = init.arguments[0] as Literal
            if (typeof sourceLiteral.value !== 'string') return

            const source = sourceLiteral.value

            if (j.Identifier.check(id)) {
                imports.push({
                    name: id.name,
                    source,
                    type: 'default',
                })
            }

            if (j.ObjectPattern.check(id)) {
                id.properties.forEach((property) => {
                    if (j.Property.check(property)
                        && j.Identifier.check(property.key)
                        && j.Identifier.check(property.value)
                    ) {
                        imports.push({
                            name: property.value.name,
                            source,
                            type: 'named',
                        })
                    }
                })
            }

            j(path).remove()
        })

    /**
     * Bare require
     *
     * @example
     * require('foo')
     */
    root
        .find(j.ExpressionStatement, {
            expression: {
                type: 'CallExpression',
                callee: {
                    type: 'Identifier',
                    name: 'require',
                },
                arguments: [{ type: 'Literal' as const }],
            },
        })
        .forEach((path) => {
            const expression = path.node.expression as CallExpression
            const sourceLiteral = expression.arguments[0] as Literal
            if (typeof sourceLiteral.value !== 'string') return

            imports.push({
                name: null,
                source: sourceLiteral.value,
                type: 'bare',
            })

            j(path).remove()
        })

    /**
     * Require with property access
     *
     * @example
     * var baz = require('baz').baz
     * var { baz } = require('baz').baz
     */
    root
        .find(j.VariableDeclaration, {
            declarations: [
                {
                    type: 'VariableDeclarator',
                    init: {
                        type: 'MemberExpression',
                        object: {
                            type: 'CallExpression',
                            callee: {
                                type: 'Identifier',
                                name: 'require',
                            },
                            arguments: [{ type: 'Literal' as const }],
                        },
                        property: {
                            type: 'Identifier',
                        },
                    },
                },
            ],
        })
        .forEach((path) => {
            const firstDeclaration = path.node.declarations[0] as VariableDeclarator
            const id = firstDeclaration.id
            const init = firstDeclaration.init as MemberExpression

            const sourceLiteral = (init.object as CallExpression).arguments[0] as Literal
            if (typeof sourceLiteral.value !== 'string') return

            const source = sourceLiteral.value

            const property = init.property as Identifier
            const propertyName = property.name

            /**
             * var baz = require('foo').bar
             * ->
             * var baz = bar
             * and add import
             */
            if (j.Identifier.check(id)) {
                imports.push({
                    name: propertyName,
                    source,
                    type: 'named',
                })

                if (id.name !== propertyName) {
                    j(path).insertAfter(j.variableDeclaration(
                        path.node.kind,
                        [
                            j.variableDeclarator(
                                id,
                                j.identifier(propertyName),
                            ),
                        ],
                    ))
                }
            }

            /**
             * var { baz } = require('foo').bar
             * ->
             * var { baz } = bar
             * and add import
             */
            if (j.ObjectPattern.check(id)) {
                imports.push({
                    name: propertyName,
                    source,
                    type: 'named',
                })

                /**
                 * Resolve name conflict
                 */
                const rootScope = root.find(j.Program).get().scope as Scope
                const bindings = rootScope?.getBindings()
                const isDeclared = bindings?.[propertyName]?.length > 0

                if (isDeclared) {
                    const newName = getUniqueName(bindings, propertyName)
                    rootScope.rename(propertyName, newName)
                }

                j(path).insertAfter(j.variableDeclaration(
                    path.node.kind,
                    [
                        j.variableDeclarator(
                            id,
                            j.identifier(propertyName),
                        ),
                    ],
                ))
            }

            j(path).remove()
        })

    /**
     * Rebuild imports
     */
    const importMap = imports.reduce((map, info) => {
        if (!map[info.source]) {
            map[info.source] = []
        }

        if (map[info.source].find(i => i.name === info.name)) return map

        map[info.source].push(info)

        return map
    }, {} as Record<string, ImportInfo[]>)

    Object.entries(importMap).forEach(([source, infos]) => {
        const importStatements: ImportDeclaration[] = []
        const variableDeclarations: VariableDeclaration[] = []

        const namedImports = infos.filter(info => info.type === 'named')
        const defaultImports = infos.filter(info => info.type === 'default')
        const namespaceImports = infos.filter(info => info.type === 'namespace')
        const bareImports = infos.filter(info => info.type === 'bare')

        if (namedImports.length > 0 || defaultImports.length > 0) {
            const importSpecifiers = [
                ...namedImports.map(info => j.importSpecifier(j.identifier(info.name!), j.identifier(info.name!))),
                ...defaultImports.map(info => j.importDefaultSpecifier(j.identifier(info.name!))),
            ]

            const importDeclaration = j.importDeclaration(
                importSpecifiers,
                j.literal(source),
            )

            importStatements.push(importDeclaration)
        }

        if (namespaceImports.length > 0) {
            const first = namespaceImports[0]
            const importDeclaration = j.importDeclaration(
                [j.importNamespaceSpecifier(j.identifier(first.name!))],
                j.literal(source),
            )
            importStatements.push(importDeclaration)

            /**
             * Other namespace imports should be aliased to the first one
             */
            if (namespaceImports.length > 1) {
                const variableDeclaration = j.variableDeclaration(
                    'const',
                    namespaceImports.slice(1).map(info => j.variableDeclarator(j.identifier(info.name!), j.identifier(first.name!))),
                )
                variableDeclarations.push(variableDeclaration)
            }
        }

        /**
         * Bare import is not needed if there are other imports
         */
        const previousImports = namedImports.length + defaultImports.length + namespaceImports.length
        if (bareImports.length > 0 && previousImports === 0) {
            const importDeclaration = j.importDeclaration(
                [],
                j.literal(source),
            )
            importStatements.push(importDeclaration)
        }

        // insert to the top of the file
        root.find(j.Program).get('body', 0).insertBefore(...importStatements)
    })
}

/**
 * Limitations:
 *
 * ```js
 * module.exports = { ... }
 * module.exports.foo = 1
 * ```
 * The above case will be transformed to:
 * ```js
 * export default { ... }
 * export const foo = 1
 * ```
 *
 * But it's technically not the same.
 */
function transformExport(context: Context) {
    const { j, root } = context

    const exportsMap = new Map<string, ASTPath>()

    function replaceWithExportDeclaration(
        j: JSCodeshift,
        path: ASTPath,
        name: string,
        right: ExpressionKind,
        kind: 'const' | 'let' | 'var' = 'const',
    ) {
        if (exportsMap.has(name)) {
            const previousPath = exportsMap.get(name)!
            previousPath.prune()
            exportsMap.delete(name)
            // removeNodeOnBody(root, previous)
            console.warn(`Multiple exports of "${name}" found, only the last one will be kept`)
        }
        exportsMap.set(name, path)

        if (name === 'default') {
            const exportDefaultDeclaration = j.exportDefaultDeclaration(right)
            j(path).replaceWith(exportDefaultDeclaration)

            return
        }

        if (j.Identifier.check(right)) {
            if (right.name === name) {
                const exportSpecifier = j.exportSpecifier.from({
                    exported: j.identifier(name),
                    local: j.identifier(name),
                })
                const exportNamedDeclaration = j.exportNamedDeclaration(
                    null,
                    [exportSpecifier],
                )

                j(path).replaceWith(exportNamedDeclaration)

                return
            }
        }
        else {
            // check if the name is declared in the scope
            // and it's not in the current path
            const rootScope = root.find(j.Program).get().scope
            const bindings = rootScope?.getBindings()
            const binding = bindings?.[name]
            const isDeclared = binding?.length > 0
            const isDeclaredInCurrentPath = binding?.some((p: NodePath) => {
                let current: ASTPath | null = p
                while (current) {
                    if (current.node === path.node) return true
                    current = current.parentPath
                }
                return false
            })
            if (isDeclared && !isDeclaredInCurrentPath) {
                // rename the existing declaration
                const oldName = name
                const newName = getUniqueName(bindings, oldName)
                console.warn(`"${name}" is already declared, rename it to "${newName}"`)
                rootScope.rename(oldName, newName)
            }
        }

        const exportNamedDeclaration = j.exportNamedDeclaration(
            j.variableDeclaration(
                kind,
                [j.variableDeclarator(j.identifier(name), right)],
            ),
            [],
        )

        j(path).replaceWith(exportNamedDeclaration)
    }

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
            const expression = path.node.expression as AssignmentExpression
            const right = expression.right

            replaceWithExportDeclaration(j, path, 'default', right)
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
            const expression = path.node.expression as AssignmentExpression
            const left = expression.left as MemberExpression
            const right = expression.right

            const name = (left.property as Identifier).name
            replaceWithExportDeclaration(j, path, name, right)
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
            const expression = path.node.expression as AssignmentExpression
            const left = expression.left as MemberExpression
            const right = expression.right

            const name = (left.property as Identifier).name
            replaceWithExportDeclaration(j, path, name, right)
        })

    /**
     * Special case:
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
            const kind = path.node.kind
            const declaration = path.node.declarations[0] as VariableDeclarator
            const id = declaration.id as Identifier
            const init = declaration.init as AssignmentExpression
            const left = init.left as MemberExpression
            const right = init.right

            const name = (left.property as Identifier).name

            if (name === 'default') {
                replaceWithExportDeclaration(j, path, name, id, kind)

                const variableDeclaration = j.variableDeclaration(
                    kind,
                    [j.variableDeclarator(id, right)],
                )

                j(path).insertBefore(variableDeclaration)

                return
            }

            replaceWithExportDeclaration(j, path, name, right, kind)

            if (id.name !== name) {
                const variableDeclaration = j.variableDeclaration(
                    kind,
                    [j.variableDeclarator(id, j.identifier(name))],
                )

                j(path).insertBefore(variableDeclaration)
            }
        })

    /**
     * Special case:
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
            const kind = path.node.kind
            const declaration = path.node.declarations[0] as VariableDeclarator
            const id = declaration.id as Identifier
            const init = declaration.init as AssignmentExpression
            const left = init.left as MemberExpression
            const right = init.right

            const name = (left.property as Identifier).name

            if (name === 'default') {
                replaceWithExportDeclaration(j, path, name, id, kind)

                const variableDeclaration = j.variableDeclaration(
                    kind,
                    [j.variableDeclarator(id, right)],
                )

                j(path).insertBefore(variableDeclaration)

                return
            }

            replaceWithExportDeclaration(j, path, name, right, kind)

            if (id.name !== name) {
                const variableDeclaration = j.variableDeclaration(
                    kind,
                    [j.variableDeclarator(id, j.identifier(name))],
                )

                j(path).insertBefore(variableDeclaration)
            }
        })

    exportsMap.clear()
}

function getUniqueName(bindings: any, oldName: string): string {
    let i = 0
    while (bindings[i > 0 ? `_${oldName}_${i}` : `_${oldName}`]) {
        i++
    }
    return i > 0 ? `_${oldName}_${i}` : `_${oldName}`
}

export default wrap(transformAST)
