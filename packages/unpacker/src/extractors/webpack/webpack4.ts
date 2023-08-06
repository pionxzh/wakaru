import { renameFunctionParameters } from '@unminify/ast-utils'
import { Module } from '../../Module'
import { convertRequireHelpersForWebpack4 } from './requireHelpers'
import type { ArrayExpression, Collection, JSCodeshift } from 'jscodeshift'

export function getModulesForWebpack4(j: JSCodeshift, root: Collection): Set<Module> | null {
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

        const moduleContent = j({ type: 'Program', body: functionExpression.body.body })
        convertRequireHelpersForWebpack4(j, moduleContent)

        const module = new Module(moduleId, moduleContent, false)
        modules.add(module)
    })

    return modules
}
