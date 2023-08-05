import type { ArrayExpression, Collection, FunctionExpression, Identifier, JSCodeshift, Literal, MemberExpression } from 'jscodeshift'
import type { ExpressionKind } from 'ast-types/gen/kinds'
import { Module } from '../Module'
import { renameFunctionParameters, wrapDeclarationWithExport } from '../utils'

export function getModules(j: JSCodeshift, root: Collection<any>): Set<Module> | null {
    /**
     * Find the modules array in webpack bootstrap
     *
     * ```js
     * (function(modules) {
     *    // webpack bootstrap
     * })([
     *    // modules
     *    function(e, t, n) {
     *       ...
     *    },
     * ])
     */
    const moduleEntry = root.find(j.CallExpression, {
        callee: {
            type: 'FunctionExpression',
            body: {
                type: 'BlockStatement',
            },
        },
        arguments: [{
            type: 'ArrayExpression' as const,
            elements: [{
                type: 'FunctionExpression' as const,
            }],
        }],
    }).at(0)
    if (!moduleEntry.size()) return null

    const modules = new Set<Module>()
    const path = moduleEntry.paths()[0]
    const arrayExpression = path.node.arguments[0] as ArrayExpression
    arrayExpression.elements.forEach((functionExpression, index) => {
        if (functionExpression?.type !== 'FunctionExpression') return
        if (functionExpression.body.type !== 'BlockStatement') return

        const moduleId = index
        renameFunctionParameters(j, functionExpression, ['module', 'exports', 'require'])
        const moduleContent = convertRequireHelpers(j, j({ type: 'Program', body: functionExpression.body.body }))
        const module = new Module(moduleId, moduleContent, false)
        modules.add(module)
    })

    return modules
}

/**
 * define `__esModule` on exports
 * makeNamespaceObject => `require.r`
 *
 * define getter functions for harmony exports
 * definePropertyGetters => `require.d`
 */
function convertRequireHelpers(j: JSCodeshift, collection: Collection<any>) {
    // remove `require.r` call as it's not needed in source code
    const requireR = collection.find(j.CallExpression, {
        callee: {
            type: 'MemberExpression',
            object: { type: 'Identifier', name: 'require' },
            property: { type: 'Identifier', name: 'r' },
        },
    })

    const isESM = requireR.size() > 0

    requireR.remove()

    /**
     * Convert `require.d(exports, key, function() { return moduleContent }` to
     *
     * ```js
     * module.exports = {
     *     default: () => (moduleContent),
     *     [key]: () => (moduleContent),
     * }
     * ```
     */
    const requireD = collection.find(j.CallExpression, {
        callee: {
            type: 'MemberExpression',
            object: { type: 'Identifier', name: 'require' },
            property: { type: 'Identifier', name: 'd' },
        },
        arguments: [{
            type: 'Identifier' as const,
            /**
             * The first argument is the exports object
             * But it's not always called `exports`
             * The common case is this `exports` object
             * is come from the function parameter
             * ```js
             * function(module, exports, require) {
             *   require.d(exports, key, function() { return moduleContent })
             * }
             * ```
             *
             * But another case is this `exports` object
             * is just an object created in the module
             * ```js
             * var exports = {}
             * require.d(exports, key, function() { return moduleContent })
             * ```
             */
            // name: 'exports' as const,
        }, {
            type: 'Literal' as const,
        }, {
            type: 'FunctionExpression' as const,
        }],
    })

    const definition = new Map<string, ExpressionKind>()
    requireD.forEach((path) => {
        const [_, key, fn] = path.node.arguments as [Identifier, Literal, FunctionExpression]

        /**
         * fn is a function expression that returns the module content
         * ```js
         * function() {
         *    return moduleContent
         * }
         * ```
         */
        if (fn.body.type !== 'BlockStatement') {
            console.warn('Unexpected module content wrapper shape:', fn.body.type)
            console.warn(j(path).toSource())
            return
        }

        const returnStatement = fn.body.body[0]
        if (returnStatement.type !== 'ReturnStatement') {
            console.warn('Unexpected module content wrapper type:', returnStatement.type)
            console.warn(j(path).toSource())
            return
        }

        const exportValue = returnStatement.argument
        if (!exportValue) {
            console.warn('Unexpected missing module content')
            console.warn(j(path).toSource())
            return
        }

        if (isESM) {
            // Find the declaration of moduleContent
            // and wrap it with `export` keyword
            if (exportValue.type === 'Identifier') {
                wrapDeclarationWithExport(j, collection, key.value as string, exportValue.name)
            }
        }
        else {
            // Convert `require.d(exports, key, function() { return moduleContent })` to
            // `module.exports = { ... }`
            definition.set(key.value as string, exportValue)
        }
    })
    requireD.remove()

    // Generate module.exports = { ... }
    if (definition.size > 0) {
        const left = j.memberExpression(j.identifier('module'), j.identifier('exports'))
        const right = j.objectExpression(Array.from(definition.entries()).map(([key, value]) => {
            return j.objectProperty(j.identifier(key), value)
        }))
        const moduleExports = j.assignmentExpression('=', left, right)
        // Add module.exports = { ... } to the end of the module
        collection.paths()[0].node.body.push(j.expressionStatement(moduleExports))
    }

    /**
     * Convert `var module0 = require(module)` to `import`
     *
     * Replace module0.property with module0_property
     */

    return collection
}
