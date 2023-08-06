import { isFunctionExpression, renameFunctionParameters } from '@unminify/ast-utils'
import { Module } from '../../Module'
import type { ModuleMapping } from './ModuleMapping'
import type { ArrayExpression, ArrowFunctionExpression, Collection, FunctionExpression, JSCodeshift, Literal, ObjectExpression } from 'jscodeshift'

/**
 * Find the modules array in browserify bootstrap.
 *
 * ```js
 * (function() {
 *   // prelude
 * })()({
 *   // [id]: [ module factory function, module map ]
 *   // module map is short require name -> numeric require(id)
 *   1: [
 *       function(require, module, exports) { ... },
 *       { "./foo": 2 }
 *   ],
 *   ...
 * },
 * { /** cache * / },
 * [ /* entry ids * /])
 * ```
 *
 * @see https://github.com/browserify/browser-pack/blob/master/prelude.js
 */
export function getModulesFromBrowserify(j: JSCodeshift, root: Collection):
{
    modules: Set<Module>
    moduleIdMapping: ModuleMapping
} | null {
    const modules = new Set<Module>()
    const moduleIdMapping = new Map<number, string>()

    const moduleDefinition = root.find(j.CallExpression, {
        callee: {
            type: 'CallExpression',
        },
        arguments: [{
            type: 'ObjectExpression' as const,
            properties: (properties: any[]) => {
                return properties.every(prop => j.Property.check(prop)
                && j.Literal.check(prop.key)
                && typeof prop.key.value === 'number'
                && j.ArrayExpression.check(prop.value)
                && prop.value.elements.length === 2
                && isFunctionExpression(j, prop.value.elements[0])
                && j.ObjectExpression.check(prop.value.elements[1])
                && prop.value.elements[1].properties.every(prop => j.Property.check(prop) && j.Literal.check(prop.key) && typeof prop.key.value === 'string' && j.Literal.check(prop.value) && typeof prop.value.value === 'number'),
                )
            },
        }, {
            type: 'ObjectExpression' as const,
        }, {
            type: 'ArrayExpression' as const,
            elements: (elements: any[]) => elements.every(el => j.Literal.check(el)),
        }],
    })

    if (!moduleDefinition.size()) return null

    moduleDefinition.forEach((path) => {
        const [modulesObject, _moduleCache, entryIdArray] = path.node.arguments as [ObjectExpression, ObjectExpression, ArrayExpression]

        const entryIds: number[] = (entryIdArray.elements as Literal[])
            .map(el => el.value as number)

        modulesObject.properties.forEach((property) => {
            if (!j.Property.check(property)) return
            if (!j.Literal.check(property.key) || typeof property.key.value !== 'number') return
            if (!j.ArrayExpression.check(property.value)) return

            const moduleId = property.key.value
            const [moduleFactory, moduleMap] = property.value.elements as [FunctionExpression | ArrowFunctionExpression, ObjectExpression]

            if (!j.BlockStatement.check(moduleFactory.body)) {
                console.warn('moduleFactory.body is not a BlockStatement', moduleFactory.body)
                return
            }

            renameFunctionParameters(j, moduleFactory, ['require', 'module', 'exports'])

            const moduleContent = j({ type: 'Program', body: moduleFactory.body.body })
            const isEntry = entryIds.includes(moduleId)
            const module = new Module(moduleId, moduleContent, isEntry)
            modules.add(module)

            moduleMap.properties.forEach((property) => {
                if (!j.Property.check(property)) return
                if (!j.Literal.check(property.key) || typeof property.key.value !== 'string') return
                if (!j.Literal.check(property.value) || typeof property.value.value !== 'number') return

                const shortName = property.key.value
                const moduleId = property.value.value

                const prevShortName = moduleIdMapping.get(moduleId)
                if (!!prevShortName && prevShortName !== shortName) {
                    console.warn(`Module ${moduleId} has multiple short names: ${prevShortName} and ${shortName}`)
                }
                moduleIdMapping.set(moduleId, shortName)
            })
        })
    })

    if (!modules.size) return null
    return { modules, moduleIdMapping }
}
