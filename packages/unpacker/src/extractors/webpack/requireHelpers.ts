import { isFunctionExpression } from '@unminify-kit/ast-utils'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { Collection, ExportDefaultDeclaration, ExportNamedDeclaration, FunctionExpression, Identifier, JSCodeshift, Literal, ObjectProperty } from 'jscodeshift'

/**
 * This function will detect the existence of `require.r`
 * and remove it from the source code.
 *
 * Return `true` if `require.r` exists.
 *
 * `require.r` is a webpack helper function
 * that defines `__esModule` on exports.
 */
export function convertRequireR(j: JSCodeshift, collection: Collection) {
    const requireR = collection.find(j.CallExpression, {
        callee: {
            type: 'MemberExpression',
            object: { type: 'Identifier', name: 'require' },
            property: { type: 'Identifier', name: 'r' },
        },
    })

    const isESM = requireR.size() > 0

    requireR.remove()

    return isESM
}

type ExportsGetterMap = Map<string, ExpressionKind>

/**
 * This function will return a map of key and module content.
 *
 * `require.d` is a webpack helper function
 * that defines getter functions for harmony exports.
 * It's used to convert ESM exports to CommonJS exports.
 *
 * Example:
 * ```js
 * require.d(exports, key, function() {
 *   return moduleContent
 * })
 * ```
 */
export function convertExportsGetterForWebpack4(j: JSCodeshift, collection: Collection): ExportsGetterMap {
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

        definition.set(key.value as string, exportValue)

        // we remove the `require.d` call one by one
        // to preserve un-supported `require.d` calls
        // for further manual inspection
        path.prune()
    })

    return definition
}

/**
 * This function will return a map of key and module content.
 *
 * `require.d` is a webpack helper function
 * that defines getter functions for harmony exports.
 * It's used to convert ESM exports to CommonJS exports.
 *
 * Example:
 * ```js
 * require.d(exports, {
 *   "default": getter,
 *   [key]: getter
 * })
 * ```
 */
export function convertExportsGetterForWebpack5(j: JSCodeshift, collection: Collection): ExportsGetterMap {
    const requireD = collection.find(j.CallExpression, {
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

    const definition = new Map<string, ExpressionKind>()
    requireD.forEach((path) => {
        const defineObject = path.node.arguments[1]
        if (defineObject.type !== 'ObjectExpression') return

        const properties = defineObject.properties as ObjectProperty[]
        properties.forEach((property) => {
            if (property.key.type !== 'Literal' && property.key.type !== 'Identifier') {
                console.warn('Unexpected export key type:', property.key.type)
                return
            }
            const exportName = ((property.key as Literal).value || (property.key as unknown as Identifier).name) as string
            if (!isFunctionExpression(j, property.value)) {
                console.warn('convertExportsGetterForWebpack5: Unexpected export value type:', property.value.type)
                return
            }
            if (j.BlockStatement.check(property.value.body)) {
                console.warn('convertExportsGetterForWebpack5: Unexpected export value body type:', property.value.body.type)
                return
            }

            definition.set(exportName, property.value.body)
        })
    })
    requireD.remove()

    return definition
}

function buildNamedExport(j: JSCodeshift, name: string, value: ExpressionKind): ExportNamedDeclaration {
    return j.exportNamedDeclaration(
        j.variableDeclaration('const', [
            j.variableDeclarator(
                j.identifier(name),
                value,
            ),
        ]),
        [],
    )
}

function buildDefaultExport(j: JSCodeshift, value: ExpressionKind): ExportDefaultDeclaration {
    return j.exportDefaultDeclaration(value)
}

export function convertExportGetter(
    j: JSCodeshift,
    collection: Collection,
    isESM: boolean,
    exportGetterMap: ExportsGetterMap,
) {
    if (isESM) {
        // Generate export { ... }
        exportGetterMap.forEach((exportValue, exportName) => {
            const exportDeclaration = exportName === 'default'
                ? buildDefaultExport(j, exportValue)
                : buildNamedExport(j, exportName, exportValue)
                // Add export { ... } to the end of the module
            collection.paths()[0].node.body.push(exportDeclaration)
        })
    }
    else {
        // Generate module.exports = { ... }
        if (exportGetterMap.size > 0) {
            const left = j.memberExpression(j.identifier('module'), j.identifier('exports'))
            const right = j.objectExpression(Array.from(exportGetterMap.entries()).map(([key, value]) => {
                return j.objectProperty(j.identifier(key), value)
            }))
            const moduleExports = j.assignmentExpression('=', left, right)
            // Add module.exports = { ... } to the end of the module
            collection.paths()[0].node.body.push(j.expressionStatement(moduleExports))
        }
    }
}

export function convertRequireHelpersForWebpack4(j: JSCodeshift, collection: Collection) {
    const isESM = convertRequireR(j, collection)
    const exportGetterMap = convertExportsGetterForWebpack4(j, collection)
    convertExportGetter(j, collection, isESM, exportGetterMap)
}

export function convertRequireHelpersForWebpack5(j: JSCodeshift, collection: Collection) {
    const isESM = convertRequireR(j, collection)
    const exportGetterMap = convertExportsGetterForWebpack5(j, collection)
    convertExportGetter(j, collection, isESM, exportGetterMap)
}
