import { isNumber, renameFunctionParameters } from '@wakaru/ast-utils'
import { Module } from '../../Module'
import { convertRequireHelpersForWebpack4 } from './requireHelpers'
import type { ModuleMapping } from '@wakaru/ast-utils'
import type { ArrayExpression, Collection, FunctionExpression, JSCodeshift, Literal } from 'jscodeshift'

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
    moduleIdMapping: ModuleMapping
} | null {
    const modules = new Set<Module>()
    const moduleIdMapping: ModuleMapping = {}

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
    if (moduleFactory.size() === 0) return null

    const path = moduleFactory.paths()[0]

    const entryIds: number[] = []
    const callee = path.node.callee as FunctionExpression

    // `require.s = 7`
    j(callee).find(j.AssignmentExpression, {
        left: {
            type: 'MemberExpression',
            object: {
                type: 'Identifier',
                // name: 'require',
            },
            property: {
                type: 'Identifier',
                name: 's',
            },
        },
        right: {
            type: 'Literal',
            value: (value: any) => isNumber(value),
        },
    }).forEach((path) => {
        entryIds.push((path.node.right as Literal).value as number)
    })

    const arrayExpression = path.node.arguments[0] as ArrayExpression
    arrayExpression.elements.forEach((functionExpression, index) => {
        if (!j.FunctionExpression.check(functionExpression)) return

        const moduleId = index
        renameFunctionParameters(j, functionExpression, ['module', 'exports', 'require'])

        const moduleContent = j({ type: 'Program', body: functionExpression.body.body })
        convertRequireHelpersForWebpack4(j, moduleContent)

        const module = new Module(moduleId, moduleContent, entryIds.includes(moduleId))
        modules.add(module)
    })

    if (modules.size === 0) return null
    return { modules, moduleIdMapping }
}
