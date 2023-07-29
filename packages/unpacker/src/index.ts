import path from 'node:path'
import fs from 'node:fs/promises'
import type { ArrowFunctionExpression, ClassDeclaration, Collection, FunctionDeclaration, FunctionExpression, Identifier, JSCodeshift, Literal, ObjectProperty, Statement, VariableDeclaration } from 'jscodeshift'
import jscodeshift from 'jscodeshift'
// @ts-expect-error - no types
import getParser from 'jscodeshift/src/getParser'
import { isIIFE, isTopLevel, prettierFormat, renameFunctionParameters } from './utils'
import { Module } from './Module'

export async function unpack() {
    const input = path.resolve('../../testcases/webpack/dist/index.js')
    const code = await fs.readFile(input, 'utf-8')
    const parser = getParser()
    const j = jscodeshift.withParser(parser)
    const root = j(code)
    const modules = new Set<Module>()

    // @ts-expect-error - Comment type is wrong
    root.find(j.Comment).forEach(path => path.prune())

    // const output = path.resolve('../../testcases/webpack/dist/output.js')
    // const formattedCode = prettierFormat(root.toSource())
    // await fs.writeFile(output, formattedCode, 'utf-8')

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

    /** Build the module map */
    // @ts-expect-error - skip type check
    const properties: ObjectProperty[] = webpackModules.declarations[0].init.properties
    properties.forEach((property) => {
        const key = (property.key as Literal).value as string
        const value = property.value as FunctionExpression | ArrowFunctionExpression
        if (value.body.type !== 'BlockStatement') return

        renameFunctionParameters(j, value, ['module', 'exports', 'require'])
        const moduleContent = j({ type: 'Program', body: value.body.body })

        const module = new Module(key, moduleContent, false)
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

    // write modules to file
    const modulesOutput = path.resolve('../../testcases/webpack/dist/modules.js')
    const modulesCode = Array.from(modules)
        .map(module => `

/**************** ${module.id} *****************/

${convertRequireHelpers(j, module.ast).toSource()}`)
        .join('\n')
    await fs.writeFile(modulesOutput, modulesCode, 'utf-8')
}

unpack()

/**
 * define `__esModule` on exports
 * makeNamespaceObject => `require.r`
 *
 * define getter functions for harmony exports
 * definePropertyGetters => `require.d`
 */
function convertRequireHelpers(j: JSCodeshift, node: Collection<any>) {
    // remove `require.r` call as it's not needed in source code
    const requireR = node.find(j.CallExpression, {
        callee: {
            type: 'MemberExpression',
            object: { type: 'Identifier', name: 'require' },
            property: { type: 'Identifier', name: 'r' },
        },
    })
    requireR.remove()

    /**
     * Convert `require.d(exports, { "default": getter, [key]: getter })` to
     *
     * ```js
     * module.exports = {
     *     default: () => (moduleContent),
     *     [key]: () => (moduleContent),
     * }
     * ```
     */
    const requireD = node.find(j.CallExpression, {
        callee: {
            type: 'MemberExpression',
            object: { type: 'Identifier', name: 'require' },
            property: { type: 'Identifier', name: 'd' },
        },
    })
    requireD.forEach((path) => {
        const defineObject = path.node.arguments[1]
        if (defineObject.type !== 'ObjectExpression') return

        const properties = defineObject.properties as ObjectProperty[]
        properties.forEach((property) => {
            if (property.key.type !== 'Literal' && property.key.type !== 'Identifier') {
                console.warn('Unexpected export key type:', property.key.type)
                return
            }
            const key = ((property.key as Literal).value || (property.key as unknown as Identifier).name) as string
            const value = property.value as unknown as FunctionExpression | ArrowFunctionExpression

            if (value.body.type !== 'Identifier') {
                console.warn('Unexpected export value type:', value.body.type)
                return
            }

            const exportName = key
            const exportValueName = value.body.name

            let declarations: Collection<VariableDeclaration> | Collection<FunctionDeclaration> | Collection<ClassDeclaration>
            declarations = node.find(j.VariableDeclaration, {
                declarations: [{
                    type: 'VariableDeclarator',
                    id: { type: 'Identifier', name: exportValueName },
                }],
            }).filter(node => isTopLevel(j, node))
            if (declarations.size() === 0) {
                declarations = node.find(j.FunctionDeclaration, {
                    id: { type: 'Identifier', name: exportValueName },
                }).filter(node => isTopLevel(j, node))
            }
            if (declarations.size() === 0) {
                declarations = node.find(j.ClassDeclaration, {
                    id: { type: 'Identifier', name: exportValueName },
                }).filter(node => isTopLevel(j, node))
            }

            if (declarations.size() === 0) {
                console.warn('Failed to locate export value:', exportValueName)
                return
            }

            const declaration = declarations.get().value
            const exportDeclaration = exportName === 'default'
                ? j.exportDefaultDeclaration(declaration)
                : j.exportNamedDeclaration(declaration)
            declarations.replaceWith(exportDeclaration)
        })
    })
    requireD.remove()

    return node
}
