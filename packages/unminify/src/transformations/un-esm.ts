import { isTopLevel } from '@unminify-kit/ast-utils'
import { generateName } from '../utils/identifier'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation, Context } from '../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { NodePath } from 'ast-types/lib/node-path'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTPath, AssignmentExpression, CallExpression, Identifier, ImportDeclaration, JSCodeshift, Literal, MemberExpression, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

type Source = string
type Imported = string
type Local = string

class ImportCollector {
    importSourceOrder = new Set<Source>()
    defaultImports = new Map<Source, Set<Local>>()
    namespaceImports = new Map<Source, Set<Local>>()
    namedImports = new Map<Source, Map<Imported, Set<Local>>>()
    bareImports = new Set<Source>()

    get importMap() {
        /**
         * Bare import can be omitted if there are other imports from the same source
         */
        const bareImports = [...this.bareImports.values()]
            .filter((source) => {
                return !this.defaultImports.has(source)
                    && !this.namespaceImports.has(source)
                    && !this.namedImports.has(source)
            })

        const importMap = [
            ...[...this.defaultImports.entries()].flatMap(([source, locals]) => [...locals].map(local => ({ type: 'default', name: local, source } as const))),
            ...[...this.namespaceImports.entries()].flatMap(([source, locals]) => [...locals].map(local => ({ type: 'namespace', name: local, source } as const))),
            ...[...this.namedImports.entries()].flatMap(([source, importedMap]) => [...importedMap.entries()].flatMap(([imported, locals]) => [...locals].map(local => ({ type: 'named', name: imported, local, source } as const)))),
            ...bareImports.map(source => ({ type: 'bare', source } as const)),
        ].reduce((map, info) => {
            if (!map[info.source]) {
                map[info.source] = []
            }

            map[info.source].push(info)

            return map
        }, {} as Record<string, ImportInfo[]>)

        const importSourceOrder = [...this.importSourceOrder.values()]
        return Object.entries(importMap).sort(([a], [b]) => {
            const aIndex = importSourceOrder.indexOf(a)
            const bIndex = importSourceOrder.indexOf(b)
            return bIndex - aIndex
        })
    }

    addImportOrder(source: string) {
        this.importSourceOrder.add(source)
    }

    addDefaultImport(source: string, local: string) {
        if (!this.defaultImports.has(source)) this.defaultImports.set(source, new Set())
        this.defaultImports.get(source)!.add(local)
    }

    addNamespaceImport(source: string, local: string) {
        if (!this.namespaceImports.has(source)) {
            this.namespaceImports.set(source, new Set())
        }
        this.namespaceImports.get(source)!.add(local)
    }

    addNamedImport(source: string, imported: string, local: string) {
        if (!this.namedImports.has(source)) {
            this.namedImports.set(source, new Map())
        }
        if (!this.namedImports.get(source)!.has(imported)) {
            this.namedImports.get(source)!.set(imported, new Set())
        }
        this.namedImports.get(source)!.get(imported)!.add(local)
    }

    addBareImport(source: string) {
        this.bareImports.add(source)
    }
}

interface DefaultImport {
    type: 'default'
    name: string
    source: string
}

interface NamespaceImport {
    type: 'namespace'
    name: string
    source: string
}

interface NamedImport {
    type: 'named'
    name: string
    local: string
    source: string
}

interface BareImport {
    type: 'bare'
    source: string
}

type ImportInfo = DefaultImport | NamespaceImport | NamedImport | BareImport

interface Params {
    hoist?: boolean
}

/**
 * Converts cjs require/exports syntax to esm import/export syntax.
 * Then combine and dedupe imports.
 *
 * @example
 * var foo = require('foo')
 * var { bar } = require('bar')
 * var bob = require('baz').baz
 * require('side-effect')
 * ->
 * import foo from 'foo'
 * import { bar } from 'bar'
 * import { baz as bob } from 'baz'
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
 * - dynamic require cannot be transformed, e.g. `require(dynamic)`
 *
 * TODO: support helper functions from bundlers
 */
function transformImport(context: Context, hoist: boolean) {
    const { root, j } = context

    /**
     * We will scan through all import and require statements
     * and collect them into a map.
     * Variable declarations will replaced in-place if needed.
     * After all, we will reconstruct the imports at the top of the file.
     */
    const importCollector = new ImportCollector()

    /**
     * Collect imports
     */
    root
        .find(j.ImportDeclaration)
        .forEach((path) => {
            const { specifiers, source } = path.node
            if (!j.Literal.check(source) || typeof source.value !== 'string') return

            const sourceValue = source.value
            importCollector.addImportOrder(sourceValue)

            if (!specifiers) {
                importCollector.addBareImport(sourceValue)
                j(path).remove()
                return
            }

            specifiers.forEach((specifier) => {
                if (j.ImportDefaultSpecifier.check(specifier)
                 && j.Identifier.check(specifier.local)) {
                    const local = specifier.local.name
                    importCollector.addDefaultImport(sourceValue, local)
                }

                if (j.ImportSpecifier.check(specifier)
                    && j.Identifier.check(specifier.imported)
                    && j.Identifier.check(specifier.local)
                ) {
                    importCollector.addNamedImport(
                        sourceValue,
                        specifier.imported.name,
                        specifier.local.name,
                    )
                }

                if (j.ImportNamespaceSpecifier.check(specifier)
                    && j.Identifier.check(specifier.local)
                ) {
                    importCollector.addNamespaceImport(
                        sourceValue,
                        specifier.local.name,
                    )
                }
            })

            j(path).remove()
        })

    /**
     * Scan through all `require` call for the recording the order of imports
     */
    root
        .find(j.CallExpression, {
            callee: {
                type: 'Identifier',
                name: 'require',
            },
            arguments: [{
                type: 'Literal' as const,
                value: (value: unknown) => typeof value === 'string',
            }],
        })
        .forEach((path) => {
            const sourceLiteral = path.node.arguments[0] as Literal
            const source = sourceLiteral.value as string
            importCollector.addImportOrder(source)
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
                        arguments: [{
                            type: 'Literal' as const,
                            value: (value: unknown) => typeof value === 'string',
                        }],
                    },
                },
            ],
        })
        .forEach((path) => {
            if (!hoist && !isTopLevel(j, path)) return

            const firstDeclaration = path.node.declarations[0] as VariableDeclarator
            const id = firstDeclaration.id
            const init = firstDeclaration.init as CallExpression

            const sourceLiteral = init.arguments[0] as Literal
            const source = sourceLiteral.value as string

            if (j.Identifier.check(id)) {
                const local = id.name
                importCollector.addDefaultImport(source, local)

                j(path).remove()
                return
            }

            /**
             * var { bar } = require('bar')
             * ->
             * import { bar } from 'bar'
             *
             */
            if (j.ObjectPattern.check(id)) {
                id.properties.forEach((property) => {
                    if (j.Property.check(property)
                        && j.Identifier.check(property.key)
                        && j.Identifier.check(property.value)
                    ) {
                        const imported = property.key.name
                        const local = property.value.name
                        importCollector.addNamedImport(source, imported, local)
                    }
                })
                j(path).remove()
                // eslint-disable-next-line no-useless-return
                return
            }
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
                arguments: [{
                    type: 'Literal' as const,
                    value: (value: unknown) => typeof value === 'string',
                }],
            },
        })
        .forEach((path) => {
            if (!hoist && !isTopLevel(j, path)) return

            const expression = path.node.expression as CallExpression
            const sourceLiteral = expression.arguments[0] as Literal
            const source = sourceLiteral.value as string
            importCollector.addBareImport(source)

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
                            arguments: [{
                                type: 'Literal' as const,
                                value: (value: unknown) => typeof value === 'string',
                            }],
                        },
                        property: {
                            type: 'Identifier',
                        },
                    },
                },
            ],
        })
        .forEach((path) => {
            if (!hoist && !isTopLevel(j, path)) return

            const firstDeclaration = path.node.declarations[0] as VariableDeclarator
            const id = firstDeclaration.id
            const init = firstDeclaration.init as MemberExpression

            const sourceLiteral = (init.object as CallExpression).arguments[0] as Literal
            const source = sourceLiteral.value as string

            const property = init.property as Identifier
            const imported = property.name

            /**
             * var baz = require('foo').bar
             * ->
             * import { bar as baz } from 'foo'
             */
            if (j.Identifier.check(id)) {
                const local = id.name
                importCollector.addNamedImport(source, imported, local)
            }

            /**
             * var { baz } = require('foo').bar
             * ->
             * import { bar } from 'foo'
             * var { baz } = bar
             */
            if (j.ObjectPattern.check(id)) {
                /**
                 * Resolve name conflict
                 *
                 * Because we are introducing a new variable `bar`,
                 * we need to make sure it doesn't conflict with
                 * existing variables.
                 */
                const local = generateName(imported, path.scope)

                importCollector.addNamedImport(source, imported, local)

                j(path).insertAfter(j.variableDeclaration(
                    path.node.kind,
                    [
                        j.variableDeclarator(
                            id,
                            j.identifier(local),
                        ),
                    ],
                ))
            }

            j(path).remove()
        })

    /**
     * All **Other** Require: Fuzzy match and replace
     *
     * @example
     * var foo = require("bar")("baz");
     * ->
     * import bar from "bar";
     * var foo = bar("baz");
     */
    if (hoist) {
        root
            .find(j.CallExpression, {
                callee: {
                    type: 'Identifier',
                    name: 'require',
                },
                arguments: [{
                    type: 'Literal' as const,
                }],
            })
            .forEach((path) => {
                const expression = path.node as CallExpression
                const sourceLiteral = expression.arguments[0] as Literal
                const source = sourceLiteral.value as string

                const moduleName = generateName(source)
                const local = generateName(moduleName, path.scope)
                j(path).replaceWith(j.identifier(local))

                importCollector.addDefaultImport(source, local)
            })
    }

    /**
     * Rebuild imports
     */
    importCollector.importMap.forEach(([source, infos]) => {
        const importStatements: ImportDeclaration[] = []
        const variableDeclarations: VariableDeclaration[] = []

        const namedImports = infos.filter(info => info.type === 'named') as NamedImport[]
        const defaultImports = infos.filter(info => info.type === 'default') as DefaultImport[]
        const namespaceImports = infos.filter(info => info.type === 'namespace') as NamespaceImport[]
        const bareImports = infos.filter(info => info.type === 'bare') as BareImport[]

        if (namedImports.length > 0 || defaultImports.length > 0) {
            const [firstDefaultImport, ...restDefaultImports] = defaultImports

            const importSpecifiers = [
                ...(firstDefaultImport ? [j.importDefaultSpecifier(j.identifier(firstDefaultImport.name))] : []),
                ...namedImports.map(info => j.importSpecifier(j.identifier(info.name), j.identifier(info.local))),
            ]
            const importDeclaration = j.importDeclaration(importSpecifiers, j.literal(source))
            importStatements.push(importDeclaration)

            if (restDefaultImports.length > 0) {
                const restImportDeclaration = restDefaultImports.map(info => j.importDeclaration(
                    [j.importDefaultSpecifier(j.identifier(info.name))],
                    j.literal(source),
                ))
                importStatements.push(...restImportDeclaration)
            }
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
        if (bareImports.length > 0) {
            const importDeclaration = j.importDeclaration([], j.literal(source))
            importStatements.push(importDeclaration)
        }

        if (variableDeclarations.length > 0) {
            root.find(j.Program).get('body', 0).insertBefore(...variableDeclarations)
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
            // TODO: handle multiple exports
        }
        exportsMap.set(name, path)

        if (name === 'default') {
            const exportDefaultDeclaration = j.exportDefaultDeclaration(right)
            j(path).replaceWith(exportDefaultDeclaration)

            return
        }

        if (j.Identifier.check(right)) {
            if (right.name === name) {
                const exported = j.identifier(name)
                const exportSpecifier = j.exportSpecifier.from({
                    exported,
                    local: exported,
                })
                const exportNamedDeclaration = j.exportNamedDeclaration(
                    null,
                    [exportSpecifier],
                )

                j(path).replaceWith(exportNamedDeclaration)

                return
            }

            /**
             * Introducing new variable `name` but conflict detected
             *
             * Go for export { right as name }
             */
            const rootScope = root.find(j.Program).get().scope as Scope | null
            if (rootScope && rootScope.declares(name)) {
                const exported = j.identifier(name)
                const exportSpecifier = j.exportSpecifier.from({
                    exported,
                    local: right,
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
            const rootScope = root.find(j.Program).get().scope as Scope | null
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
                /**
                 * Resolve name conflict
                 * Because we are introducing a new variable
                 * but the name is already declared in the scope
                 *
                 * @example
                 * const foo = 1
                 * module.exports.foo = 2
                 * ->
                 * const foo = 1
                 * const foo$0 = 2
                 * export { foo$0 as foo }
                 */
                const oldName = name
                const newName = generateName(oldName, path.scope)

                const variableDeclaration = j.variableDeclaration(
                    kind,
                    [j.variableDeclarator(j.identifier(newName), right)],
                )
                j(path).insertBefore(variableDeclaration)

                const exportSpecifier = j.exportSpecifier.from({
                    exported: j.identifier(name),
                    local: j.identifier(newName),
                })
                const exportNamedDeclaration = j.exportNamedDeclaration(
                    null,
                    [exportSpecifier],
                )
                j(path).replaceWith(exportNamedDeclaration)
                return
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
            if (!isTopLevel(j, path)) return

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
            if (!isTopLevel(j, path)) return

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
            if (!isTopLevel(j, path)) return

            const expression = path.node.expression as AssignmentExpression
            const left = expression.left as MemberExpression
            const right = expression.right

            const name = (left.property as Identifier).name
            replaceWithExportDeclaration(j, path, name, right)
        })

    /**
     * Special case:
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

export default wrap(transformAST)
