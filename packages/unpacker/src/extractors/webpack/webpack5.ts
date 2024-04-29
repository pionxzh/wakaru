import { renameFunctionParameters } from '@wakaru/ast-utils'
import { isStatementIIFE } from '@wakaru/ast-utils/matchers'
import { getTopLevelStatements } from '@wakaru/ast-utils/program'
import { Module } from '../../Module'
import { convertRequireHelpersForWebpack5 } from './requireHelpers'
import type { ModuleMapping } from '@wakaru/ast-utils/types'
import type { ArrowFunctionExpression, CallExpression, Collection, ExpressionStatement, FunctionExpression, JSCodeshift, ObjectProperty, Statement, StringLiteral, UnaryExpression, VariableDeclaration } from 'jscodeshift'

/**
 * Find the modules map in webpack 5 bootstrap.
 *
 * Webpack 5 Bundle Structure
 *
 * ```js
 * (() => { // webpackBootstrap
 *   var __webpack_modules__ = ({
 *     "{path}": ((__unused_webpack___webpack_module__, __webpack_exports__, __webpack_require__) => {
 *        // module content...
 *     }),
 *     ...
 *   })
 *  var __webpack_module_cache__ = {}
 *
 *  // Webpack Runtime...
 *
 *  // Entry Module...
 *
 *  // simple entry
 *  __webpack_require__("{path}")
 *
 *  // or complex entry
 *
 *  // This entry need to be wrapped in an IIFE because it need to be isolated against other modules in the chunk.
 *  (() => {
 *    // entry module content...
 *  })
 * })
 * ```
 */
export function getModulesForWebpack5(j: JSCodeshift, root: Collection):
{
    modules: Set<Module>
    moduleIdMapping: ModuleMapping
} | null {
    const modules = new Set<Module>()
    const moduleIdMapping: ModuleMapping = {}

    const statements = getTopLevelStatements(root)
    const webpackBootstrap = statements.find(node => isStatementIIFE(j, node)) as ExpressionStatement | undefined
    if (!webpackBootstrap) return null

    const expression = webpackBootstrap.expression as CallExpression | UnaryExpression
    const callExpression = (j.CallExpression.check(expression) ? expression : expression.argument) as CallExpression
    const callee = callExpression.callee as FunctionExpression | ArrowFunctionExpression
    if (!j.BlockStatement.check(callee.body)) return null

    const statementsInBootstrap: Statement[] = callee.body.body
    const webpackModules = statementsInBootstrap.find((node) => {
        if (node.type !== 'VariableDeclaration') return false

        const declaration = (node as VariableDeclaration).declarations[0]
        if (declaration.type !== 'VariableDeclarator') return false
        if (declaration.init?.type !== 'ObjectExpression') return false

        const properties = declaration.init.properties as ObjectProperty[]
        if (properties.length === 0) return false
        return properties.every((property) => {
            return j.StringLiteral.check(property.key)
                && (
                    j.FunctionExpression.check(property.value)
                 || j.ArrowFunctionExpression.check(property.value)
                )
        })
    })
    if (!webpackModules) return null

    /** Build the module map */
    // @ts-expect-error - skip type check
    const properties: ObjectProperty[] = webpackModules.declarations[0].init.properties
    properties.forEach((property) => {
        const moduleId = (property.key as StringLiteral).value as string
        const functionExpression = property.value as FunctionExpression | ArrowFunctionExpression
        if (functionExpression.body.type !== 'BlockStatement') return

        renameFunctionParameters(j, functionExpression, ['module', 'exports', 'require'])

        const program = j.program(functionExpression.body.body)
        if (functionExpression.body.directives) {
            program.directives = [...(program.directives || []), ...functionExpression.body.directives]
        }
        const moduleContent = j(program)
        convertRequireHelpersForWebpack5(j, moduleContent)

        const module = new Module(moduleId, moduleContent, false)
        modules.add(module)
    })

    /** Build the entry module */
    const lastStatement = statementsInBootstrap[statementsInBootstrap.length - 1]
    if (isStatementIIFE(j, lastStatement)) {
        // @ts-expect-error - skip type check
        const functionExpression = lastStatement.expression.callee
        const program = j.program(functionExpression.body.body)
        if (functionExpression.body.directives) {
            program.directives = [...(program.directives || []), ...functionExpression.body.directives]
        }
        const moduleContent = j(program)
        const module = new Module('entry.js', moduleContent, true)
        modules.add(module)
    }
    else {
        // TODO: find a proper way to split the entry module
        // throw new Error('Entry module is not an IIFE')
        console.warn('Entry module is not an IIFE')
    }

    if (modules.size === 0) return null
    return { modules, moduleIdMapping }
}
