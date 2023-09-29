import { renameFunctionParameters } from '@unminify-kit/ast-utils'
import { Module } from '../../Module'
import { convertRequireHelpersForWebpack4, convertRequireHelpersForWebpack5 } from './requireHelpers'
import type { ModuleMapping } from '../../ModuleMapping'
import type { ArrayExpression, Collection, FunctionExpression, JSCodeshift, Literal, MemberExpression, ObjectExpression, Property } from 'jscodeshift'

/**
 * Find the modules array in webpack jsonp chunk.
 *
 * @example
 * (self.webpackChunk_N_E=self.webpackChunk_N_E || []).push([[888],{2189: ...}
 *
 * @example
 * (window["webpackJsonp"] = window["webpackJsonp"] || []).push(chunkIds, moreModules)
 */
export function getModulesForWebpackJsonP(j: JSCodeshift, root: Collection):
{
    modules: Set<Module>
    moduleIdMapping: ModuleMapping
} | null {
    const modules = new Set<Module>()
    const moduleIdMapping: ModuleMapping = {}

    /**
     * jsonpFunction can set by `output.jsonpFunction` in webpack4
     * or `output.chunkLoadingGlobal` in webpack5
     *
     * `webpackJsonp` is used in webpack 4
     * `webpackChunk_N_E` is used in Next.js
     */

    const selfVariableNames = ['self', 'window']

    const moduleFactory = root.find(j.CallExpression, {
        callee: {
            type: 'MemberExpression',
            object: {
                type: 'AssignmentExpression',
                left: {
                    type: 'MemberExpression',
                    object: {
                        type: 'Identifier',
                        name: (name: string) => selfVariableNames.includes(name),
                    },
                    property: (property: MemberExpression['property']) => {
                        return j.Identifier.check(property) || j.Literal.check(property)
                    },
                },
                right: {
                    type: 'LogicalExpression',
                    operator: '||',
                    left: {
                        type: 'MemberExpression',
                        object: {
                            type: 'Identifier',
                            name: (name: string) => selfVariableNames.includes(name),
                        },
                        property: (property: MemberExpression['property']) => {
                            return j.Identifier.check(property) || j.Literal.check(property)
                        },
                    },
                    right: {
                        type: 'ArrayExpression',
                        elements: (elements: ArrayExpression['elements']) => {
                            return elements.length === 0
                        },
                    },
                },
            },
            property: {
                type: 'Identifier',
                name: 'push',
            },
        },
        arguments: [{
            type: 'ArrayExpression',
            elements: [
                { type: 'ArrayExpression' } as const,
                {
                    type: 'ObjectExpression',
                    properties: (properties: ObjectExpression['properties']) => {
                        if (properties.length === 0) return false
                        return properties.every((property) => {
                            return j.Property.check(property)
                                    && j.Literal.check(property.key)
                                    && j.FunctionExpression.check(property.value)
                        })
                    },
                },
            ],
        }],
    })
    if (moduleFactory.size() === 0) return null

    moduleFactory.forEach((path) => {
        const [arrayExpression] = path.node.arguments as [ArrayExpression]
        const [_chunkIds, moreModules] = arrayExpression.elements as [ArrayExpression, ObjectExpression, any]

        moreModules.properties.forEach((property) => {
            const prop = property as Property
            const moduleId = (prop.key as Literal).value
            if (typeof moduleId !== 'number' && typeof moduleId !== 'string') return
            const functionExpression = prop.value as FunctionExpression
            renameFunctionParameters(j, functionExpression, ['module', 'exports', 'require'])

            const moduleContent = j({ type: 'Program', body: functionExpression.body.body })
            convertRequireHelpersForWebpack4(j, moduleContent)
            convertRequireHelpersForWebpack5(j, moduleContent)

            const module = new Module(moduleId, j, moduleContent, false)
            modules.add(module)
        })
    })

    if (modules.size === 0) return null
    return { modules, moduleIdMapping }
}
