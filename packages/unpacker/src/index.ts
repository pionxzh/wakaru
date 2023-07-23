import path from 'node:path'
import fs from 'node:fs/promises'
import type { ASTPath, ArrowFunctionExpression, ExpressionStatement, FunctionExpression, Literal, ObjectProperty, Statement, VariableDeclaration } from 'jscodeshift'
import jscodeshift from 'jscodeshift'
// @ts-expect-error - no types
import getParser from 'jscodeshift/src/getParser'
import { renameFunctionParameters } from './utils'
import { Module } from './Module'

export default async function unpack() {
    const input = path.resolve('../../testcases/webpack/dist/index.js')
    const code = await fs.readFile(input, 'utf-8')
    const parser = getParser()
    const j = jscodeshift.withParser(parser)
    const root = j(code)
    const modules = new Set<Module>()

    // @ts-expect-error - Comment type is wrong
    root.find(j.Comment).forEach(path => path.prune())

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
    const webpackBootstrap = body.find((node) => {
        if (node.type === 'ExpressionStatement') {
            const expression = (node as ExpressionStatement).expression
            if (expression.type === 'CallExpression') {
                const callee = expression.callee
                return callee.type === 'FunctionExpression'
                    || callee.type === 'ArrowFunctionExpression'
            }
        }
        return false
    })
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

    /** Build the module map */
    // @ts-expect-error - skip type check
    const properties: ObjectProperty[] = webpackModules.declarations[0].init.properties
    properties.forEach((property) => {
        const key = (property.key as Literal).value as string
        const value = property.value as FunctionExpression | ArrowFunctionExpression
        if (value.body.type !== 'BlockStatement') return

        renameFunctionParameters(j, value, ['module', 'exports', 'require'])
        const moduleContent = j(value.body.body)
        // console.log(key, moduleContent.toSource())
        const module = new Module(key, moduleContent, false)
        modules.add(module)
    })
}

unpack()
