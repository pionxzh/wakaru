import { isTopLevel } from '@wakaru/ast-utils'
import { mergeComments } from '@wakaru/ast-utils/comments'
import { generateName, isValidIdentifier } from '@wakaru/ast-utils/identifier'
import { ImportManager } from '@wakaru/ast-utils/imports'
import { isExportObject, isStringObjectProperty, isUndefined } from '@wakaru/ast-utils/matchers'
import { getNodePosition } from '@wakaru/ast-utils/position'
import { findReferences, renameIdentifier } from '@wakaru/ast-utils/reference'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import { z } from 'zod'
import { transformAST as interopRequireDefault } from './runtime-helpers/babel/interopRequireDefault'
import { NAMESPACE_IMPORT_HINT, transformAST as interopRequireWildcard } from './runtime-helpers/babel/interopRequireWildcard'
import type { ASTTransformation, Context } from '@wakaru/shared/rule'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { NodePath } from 'ast-types/lib/node-path'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTNode, ASTPath, AssignmentExpression, CallExpression, Identifier, JSCodeshift, MemberExpression, Node, NumericLiteral, StringLiteral, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

const Schema = z.object({
    hoist: z.boolean().default(false).describe('Hoist non-top-level require calls to the top of the file'),
})

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
export const transformAST: ASTTransformation<typeof Schema> = (context, params) => {
    Schema.parse(params)

    // handle interop
    interopRequireDefault(context, params)
    interopRequireWildcard(context, params)

    transformImport(context, params.hoist)
    transformExport(context)
}

/**
 * Limitations:
 * - dynamic require cannot be transformed, e.g. `require(dynamic)`
 */
function transformImport(context: Context, hoist: boolean) {
    const { root, j } = context

    /**
     * We will scan through all import and require statements
     * and collect them into a map.
     * Variable declarations will replaced in-place if needed.
     * After all, we will reconstruct the imports at the top of the file.
     */
    const importManager = new ImportManager()
    importManager.collectEsModuleImport(j, root)

    root
        .find(j.CallExpression, {
            callee: {
                type: 'Identifier',
                name: 'require',
            },
            arguments: (args) => {
                if (args.length !== 1) return false
                return j.StringLiteral.check(args[0])
            },
        })
        .forEach((path) => {
            const sourceLiteral = path.node.arguments[0] as StringLiteral
            const source = sourceLiteral.value
            importManager.addImportOrder(source)

            const parentPath = path.parent as ASTPath

            if (j.ExpressionStatement.check(parentPath.node)) {
                handleBareRequire(parentPath, source)
                return
            }

            if (
                j.VariableDeclarator.check(parentPath.node)
             && parentPath.node.init === path.node
            ) {
                const isNamespace = isNamespaceImport(path)
                handleBasicRequire(parentPath as ASTPath<VariableDeclarator>, source, isNamespace)
                return
            }

            if (
                j.MemberExpression.check(parentPath.node)
             && parentPath.node.object === path.node
             && j.VariableDeclarator.check(parentPath.parent.node)
             && j.VariableDeclaration.check(parentPath.parent.parent.node)
            ) {
                handleRequireWithPropertyAccess(parentPath as ASTPath<MemberExpression>, source)
                return
            }

            handleDynamicRequire(path, source)

            if (hoist) {
                handleFuzzyRequire(path, source)
            }
        })

    handleNamespaceImport()

    handleMissingModuleRequire()

    importManager.applyImportToRoot(j, root)

    /**
     * Bare require
     *
     * @example
     * require('foo')
     */
    function handleBareRequire(path: ASTPath, source: string) {
        if (!checkHoistable(j, path, hoist)) return false

        importManager.addBareImport(source)
        path.prune()
        return true
    }

    /*
    * Basic require and require with destructuring
    *
    * @example
    * var foo = require('foo')
    * var { bar } = require('bar')
    */
    function handleBasicRequire(path: ASTPath<VariableDeclarator>, source: string, isNamespace: boolean) {
        if (!checkHoistable(j, path.parent, hoist)) return false

        const id = path.node.id

        if (j.Identifier.check(id)) {
            const local = id.name
            if (isNamespace) importManager.addNamespaceImport(source, local)
            else importManager.addDefaultImport(source, local)

            path.parent.prune()
            return true
        }

        if (j.ObjectPattern.check(id)) {
            id.properties.forEach((property) => {
                if (j.ObjectProperty.check(property) && j.Identifier.check(property.key) && j.Identifier.check(property.value)) {
                    const imported = property.key.name
                    const local = property.value.name
                    importManager.addNamedImport(source, imported, local)
                }
            })
            path.parent.prune()
            return true
        }

        return false
    }

    /**
     * Require with property access
     *
     * @example
     * var baz = require('baz').baz
     * var baz = require('baz').default
     * var { baz } = require('baz').baz
     * var { baz } = require('baz').default
     */
    function handleRequireWithPropertyAccess(path: ASTPath<MemberExpression>, source: string) {
        const variableDeclarationPath = path.parent.parent as ASTPath<VariableDeclaration>
        if (!checkHoistable(j, variableDeclarationPath, hoist)) return

        const variableDeclarator = path.parent.node as VariableDeclarator
        const id = variableDeclarator.id
        const init = path.node
        const property = init.property

        let imported: string | null = null
        if (init.computed && j.StringLiteral.check(property)) imported = property.value
        else if (!init.computed && j.Identifier.check(property)) imported = property.name
        if (!imported) return
        if (imported !== 'default' && !isValidIdentifier(imported)) return

        if (j.Identifier.check(id)) {
            const local = id.name

            if (imported === 'default') importManager.addDefaultImport(source, local)
            else importManager.addNamedImport(source, imported, local)

            variableDeclarationPath.prune()
            return
        }

        /**
         * var { baz } = require('foo').bar
         * ->
         * import { bar } from 'foo'
         * var { baz } = bar
         */
        if (j.ObjectPattern.check(id)) {
            if (imported === 'default') {
                id.properties.forEach((property) => {
                    if (j.ObjectProperty.check(property)
                        && j.Identifier.check(property.key)
                        && j.Identifier.check(property.value)
                    ) {
                        const imported = property.key.name
                        const local = property.value.name
                        importManager.addNamedImport(source, imported, local)
                    }
                })
                variableDeclarationPath.prune()
                return
            }

            /**
             * Resolve name conflict
             *
             * Because we are introducing a new variable `bar`,
             * we need to make sure it doesn't conflict with
             * existing variables.
             */
            const local = generateName(imported, path.scope)
            importManager.addNamedImport(source, imported, local)

            j(variableDeclarationPath).insertAfter(j.variableDeclaration(
                variableDeclarationPath.node.kind,
                [j.variableDeclarator(id, j.identifier(local))],
            ))

            variableDeclarationPath.prune()
        }
    }

    function handleDynamicRequire(path: ASTPath<CallExpression>, source: string) {
        // Promise.resolve().then(() => require('foo'));
        if (
            j.match(path.parent.parent.node, {
                type: 'CallExpression',
                callee: {
                    type: 'MemberExpression',
                    object: {
                        type: 'CallExpression',
                        callee: {
                            type: 'MemberExpression',
                            object: {
                                type: 'Identifier',
                                name: 'Promise',
                            },
                            property: {
                                type: 'Identifier',
                                name: 'resolve',
                            },
                        },
                        arguments: [],
                    },
                    property: {
                        type: 'Identifier',
                        name: 'then',
                    },
                },
                arguments: [{
                    type: 'ArrowFunctionExpression',
                    params: [],
                    // @ts-expect-error
                    body: (body: any) => body === path.node,
                }],
            })
        ) {
            const dynamicImport = j.importExpression(j.stringLiteral(source))
            path.parent.parent.replace(dynamicImport)
        }
    }

    /**
     * All **Other** Require: Fuzzy match and replace
     *
     * @example
     * var foo = require("bar")("baz");
     * ->
     * import bar from "bar";
     * var foo = bar("baz");
     */
    function handleFuzzyRequire(path: ASTPath, source: string) {
        const local = generateName(source, path.scope)
        path.replace(j.identifier(local))
        importManager.addDefaultImport(source, local)
    }

    /**
     * Find all default imports that are actually namespace imports
     * and convert them to namespace imports.
     */
    function handleNamespaceImport() {
        const rootScope = root.find(j.Program).get().scope as Scope | null
        if (rootScope) {
            importManager.defaultImports.forEach((locals, source) => {
                locals.forEach((local) => {
                    findReferences(j, rootScope, local).some((path) => {
                        if (!isNamespaceImport(path)) return false

                        const parentPath = path.parent as ASTPath

                        /**
                         * var _bar = require("bar");
                         * var _source = _interopRequireWildcard(_bar);
                         * ->
                         * import * as _source from "bar";
                         */
                        if (
                            j.VariableDeclarator.check(parentPath.node)
                             && j.Identifier.check(parentPath.node.id)
                             && parentPath.node.init === path.node
                        ) {
                            const variableDeclarator = parentPath.node as VariableDeclarator
                            const id = variableDeclarator.id as Identifier

                            renameIdentifier(j, rootScope, local, id.name)

                            importManager.addNamespaceImport(source, id.name)
                            importManager.removeDefaultImport(source, local)
                            parentPath.prune()

                            return true
                        }

                        /**
                         * var _bar = require("bar");
                         * _source = _interopRequireWildcard(_bar);
                         * ->
                         * import * as _source from "bar";
                         */
                        if (
                            j.AssignmentExpression.check(parentPath.node)
                             && j.Identifier.check(parentPath.node.left)
                             && parentPath.node.right === path.node
                        ) {
                            const assignmentExpression = parentPath.node as AssignmentExpression
                            const id = assignmentExpression.left as Identifier

                            renameIdentifier(j, rootScope, local, id.name)

                            importManager.addNamespaceImport(source, id.name)
                            importManager.removeDefaultImport(source, local)
                            parentPath.prune()

                            return true
                        }

                        return false
                    })
                })
            })
        }
    }

    /**
     * Add /* wakaru:missing / annotation to require calls that cannot be transformed.
     */
    function handleMissingModuleRequire() {
        root
            .find(j.CallExpression, {
                callee: {
                    type: 'Identifier',
                    name: 'require',
                },
                arguments: (args) => {
                    if (args.length !== 1) return false
                    return j.NumericLiteral.check(args[0])
                },
            })
            .forEach((path) => {
                const sourcePath = path.get('arguments', 0) as ASTPath<NumericLiteral>
                const comment = j.commentBlock(' wakaru:missing ', false, true)
                mergeComments(sourcePath.node, [comment])
            })
    }
}

function checkHoistable(j: JSCodeshift, path: ASTPath, hoist: boolean) {
    return hoist || isTopLevel(j, path)
}

function isNamespaceImport(path: ASTPath<Node>) {
    return path.node.comments?.some(comment => comment.value === NAMESPACE_IMPORT_HINT) ?? false
}

/**
 * Known limitations:
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
 * But it's technically not correct.
 */
function transformExport(context: Context) {
    const { j, root } = context

    const exportsMap = new Map<string, { path: ASTPath; callback: () => void }>()

    /**
     * We collect all exports and do the pre-deduplication.
     * Then, all exports will be processed at the end of the file.
     */
    function enqueueExport(
        j: JSCodeshift,
        name: string,
        path: ASTPath,
        callback: () => void,
    ) {
        /**
         * Multiple exports of the same name detected.
         * We will keep the last one and prune the previous one.
         */
        if (exportsMap.has(name)) {
            // if the current export is before the previous export, we can safely ignore it
            const prevPos = getNodePosition(exportsMap.get(name)!.path.node)
            const currPos = getNodePosition(path.node)
            if ((currPos?.start ?? 0) < (prevPos?.start ?? 0)) {
                path.prune()
                return
            }

            /**
             * If the current path is referencing the previous export,
             * we can simply remove the current path.
             *
             * @example
             * module.exports = foo;
             * module.exports.default = module.exports;
             */
            if (name === 'default' && j.match(path.node, {
                type: 'ExpressionStatement',
                // @ts-expect-error skip check left part
                expression: {
                    type: 'AssignmentExpression',
                    operator: '=',
                    right: {
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
            })) {
                path.prune()
                return
            }

            const { path: existingPath } = exportsMap.get(name)!

            /**
             * Babel and TypeScript will assign `void 0` to `exports`
             * before assigning the actual value.
             *
             * @example
             * exports.foo = void 0
             * exports.foo = 1
             *
             * So we can safely mute the multi-export warning for this case.
             */
            const shouldIgnoreMultipleExports = isInitializationExport(j, existingPath.node)
            if (!shouldIgnoreMultipleExports) {
                console.warn(`[${context.filename}] Multiple exports of "${name}" found, only the last one will be kept`)
            }

            exportsMap.delete(name)
            existingPath.prune()
        }

        exportsMap.set(name, { path, callback })
    }

    function replaceWithExportDeclaration(
        j: JSCodeshift,
        path: ASTPath,
        name: string,
        right: ExpressionKind,
        kind: 'const' | 'let' | 'var' = 'const',
    ) {
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

                rootScope.markAsStale()

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
                    current = current.parent
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

                rootScope?.markAsStale()
                path.scope?.markAsStale()

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

            enqueueExport(j, 'default', path, () => {
                replaceWithExportDeclaration(j, path, 'default', right)
            })
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
                    object: (node: MemberExpression['object']) => isExportObject(j, node),
                    property: (node: MemberExpression['property']) => isStringObjectProperty(j, node),
                },
            },
        })
        .forEach((path) => {
            if (!isTopLevel(j, path)) return

            const expression = path.node.expression as AssignmentExpression
            const left = expression.left as MemberExpression
            const right = expression.right

            const property = left.property as Identifier | StringLiteral
            const name = j.Identifier.check(property) ? property.name : property.value
            enqueueExport(j, name, path, () => {
                replaceWithExportDeclaration(j, path, name, right)
            })
        })

    /**
     * This pattern is introduced by Babel in https://github.com/babel/babel/pull/15984
     *
     * @example
     * var foo = exports.foo = 1
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
                            object: (node: MemberExpression['object']) => isExportObject(j, node),
                            property: (node: MemberExpression['property']) => isStringObjectProperty(j, node),
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

            const property = left.property as Identifier | StringLiteral
            const name = j.Identifier.check(property) ? property.name : property.value

            if (name === 'default') {
                enqueueExport(j, name, path, () => {
                    replaceWithExportDeclaration(j, path, name, id, kind)

                    const variableDeclaration = j.variableDeclaration(
                        kind,
                        [j.variableDeclarator(id, right)],
                    )

                    j(path).insertBefore(variableDeclaration)
                })

                return
            }

            enqueueExport(j, name, path, () => {
                replaceWithExportDeclaration(j, path, name, right, kind)

                if (id.name !== name) {
                    const variableDeclaration = j.variableDeclaration(
                        kind,
                        [j.variableDeclarator(id, j.identifier(name))],
                    )

                    j(path).insertBefore(variableDeclaration)
                }
            })
        })

    exportsMap.forEach(({ callback }) => callback())
}

/**
 * Babel and TypeScript will assign `void 0` to `exports` before assigning the actual value.
 *
 * This is called "initialization statements".
 *
 * @see https://github.com/babel/babel/blob/main/packages/babel-helper-module-transforms/src/index.ts
 */
function isInitializationExport(j: JSCodeshift, node: ASTNode) {
    // exports.foo = void 0
    return j.ExpressionStatement.check(node)
        && j.AssignmentExpression.check(node.expression)
        && isUndefined(j, node.expression.right)
}

export default createJSCodeshiftTransformationRule({
    name: 'un-esm',
    transform: transformAST,
    schema: Schema,
})
