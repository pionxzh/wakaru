import { isIIFE, renameFunctionParameters } from '@unminify/ast-utils'
import { Module } from '../../Module'
import { convertRequireHelpersForWebpack5 } from './requireHelpers'
import type { ArrowFunctionExpression, Collection, FunctionExpression, JSCodeshift, Literal, ObjectProperty, Statement, VariableDeclaration } from 'jscodeshift'

export function getModulesForWebpack5(j: JSCodeshift, root: Collection<any>): Set<Module> | null {
    /**
     * Webpack 5 Bundle Structure
     *
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
     * // Entry Module...
     *
     * // will be like this if this part only contains module require
     * __webpack_require__("{path}")
     *
     * // or
     *
     * // This entry need to be wrapped in an IIFE because it need to be isolated against other modules in the chunk.
     * (() => {
     *   // entry module content...
     * })
     */
    const body = root.get().node.program.body as Statement[]
    const webpackBootstrap = body.find(node => isIIFE(node))
    if (!webpackBootstrap) return null

    // @ts-expect-error - skip type check
    const statementsInBootstrap: Statement[] = webpackBootstrap.expression.callee.body.body
    const webpackModules = statementsInBootstrap.find((node) => {
        if (node.type !== 'VariableDeclaration') return false

        const declaration = (node as VariableDeclaration).declarations[0]
        if (declaration.type !== 'VariableDeclarator') return false
        if (declaration.init?.type !== 'ObjectExpression') return false

        const properties = declaration.init.properties as ObjectProperty[]
        if (properties.length === 0) return false
        return properties.every((property) => {
            return property.key.type === 'Literal'
                && (
                    property.value.type === 'FunctionExpression'
                 || property.value.type === 'ArrowFunctionExpression'
                )
        })
    })
    if (!webpackModules) return null

    const modules = new Set<Module>()

    /** Build the module map */
    // @ts-expect-error - skip type check
    const properties: ObjectProperty[] = webpackModules.declarations[0].init.properties
    properties.forEach((property) => {
        const moduleId = (property.key as Literal).value as string
        const functionExpression = property.value as FunctionExpression | ArrowFunctionExpression
        if (functionExpression.body.type !== 'BlockStatement') return

        renameFunctionParameters(j, functionExpression, ['module', 'exports', 'require'])

        const moduleContent = j({ type: 'Program', body: functionExpression.body.body })
        convertRequireHelpersForWebpack5(j, moduleContent)

        const module = new Module(moduleId, moduleContent, false)
        modules.add(module)
    })

    /** Build the entry module */
    const lastStatement = statementsInBootstrap[statementsInBootstrap.length - 1]
    if (isIIFE(lastStatement)) {
        // @ts-expect-error - skip type check
        const entryModule = lastStatement.expression.callee.body.body
        const moduleContent = j({ type: 'Program', body: entryModule })
        const module = new Module('entry.js', moduleContent, true)
        modules.add(module)
    }
    else {
        // TODO: find a proper way to split the entry module
        // throw new Error('Entry module is not an IIFE')
        console.warn('Entry module is not an IIFE')
    }

    return modules
}
