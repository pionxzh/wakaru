import { renameFunctionParameters } from '@unminify/ast-utils'
import { Module } from '../../Module'
import { convertRequireHelpersForWebpack4 } from './requireHelpers'
import type { ArrayExpression, Collection, JSCodeshift } from 'jscodeshift'

/**
 * Find the modules array in webpack bootstrap
 *
 * ```js
 * (function(modules) {
 *    // webpack bootstrap
 * })([
 *    // module factory
 *    function(e, t, n) {
 *       ...
 *    },
 * ])
 * ```
 */
export function getModulesForWebpack4(j: JSCodeshift, root: Collection):
{
    modules: Set<Module>
    moduleIdMapping: Map<number, string>
} | null {
    const modules = new Set<Module>()
    const moduleIdMapping = new Map<number, string>()

    const moduleFactory = root.find(j.CallExpression, {
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
    if (!moduleFactory.size()) return null

    const path = moduleFactory.paths()[0]
    const arrayExpression = path.node.arguments[0] as ArrayExpression
    arrayExpression.elements.forEach((functionExpression, index) => {
        if (functionExpression?.type !== 'FunctionExpression') return
        if (functionExpression.body.type !== 'BlockStatement') return

        const moduleId = index
        renameFunctionParameters(j, functionExpression, ['module', 'exports', 'require'])

        const moduleContent = j({ type: 'Program', body: functionExpression.body.body })
        convertRequireHelpersForWebpack4(j, moduleContent)

        const module = new Module(moduleId, moduleContent, false)
        modules.add(module)
    })

    // TODO: detect entry point
    // `require.s = 7` is the entry point

    if (modules.size === 0) return null
    return { modules, moduleIdMapping }
}
