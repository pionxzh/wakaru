import { isFunctionExpression, renameFunctionParameters } from '@wakaru/ast-utils'
import { Module } from '../../Module'
import type { ModuleMapping } from '@wakaru/ast-utils'
import type { ArrayExpression, ArrowFunctionExpression, Collection, FunctionExpression, JSCodeshift, NumericLiteral, ObjectExpression } from 'jscodeshift'

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
    const moduleIdMapping: ModuleMapping = {}

    const moduleDefinition = root.find(j.CallExpression, {
        callee: {
            type: 'CallExpression',
        },
        arguments: [{
            type: 'ObjectExpression' as const,
            properties: (properties: any[]) => {
                return properties.every(prop => j.ObjectProperty.check(prop)
                && j.NumericLiteral.check(prop.key)
                && j.ArrayExpression.check(prop.value)
                && prop.value.elements.length === 2
                && isFunctionExpression(j, prop.value.elements[0])
                && j.ObjectExpression.check(prop.value.elements[1])
                && prop.value.elements[1].properties.every(prop => j.ObjectProperty.check(prop) && j.StringLiteral.check(prop.key) && j.NumericLiteral.check(prop.value)),
                )
            },
        }, {
            type: 'ObjectExpression' as const,
        }, {
            type: 'ArrayExpression' as const,
            elements: (elements: any[]) => elements.every(el => j.NumericLiteral.check(el)),
        }],
    })

    if (!moduleDefinition.size()) return null

    moduleDefinition.forEach((path) => {
        const [modulesObject, _moduleCache, entryIdArray] = path.node.arguments as [ObjectExpression, ObjectExpression, ArrayExpression]

        const entryIds: number[] = (entryIdArray.elements as NumericLiteral[]).map(el => el.value)

        modulesObject.properties.forEach((property) => {
            if (!j.ObjectProperty.check(property)) return
            if (!j.NumericLiteral.check(property.key)) return
            if (!j.ArrayExpression.check(property.value)) return

            const moduleId = property.key.value
            const [moduleFactory, moduleMap] = property.value.elements as [FunctionExpression | ArrowFunctionExpression, ObjectExpression]

            if (!j.BlockStatement.check(moduleFactory.body)) {
                console.warn('moduleFactory.body is not a BlockStatement', moduleFactory.body)
                return
            }

            renameFunctionParameters(j, moduleFactory, ['require', 'module', 'exports'])

            const program = j.program(moduleFactory.body.body)
            if (moduleFactory.body.directives) {
                program.directives = [...(program.directives || []), ...moduleFactory.body.directives]
            }
            const moduleContent = j(program)
            const isEntry = entryIds.includes(moduleId)
            const module = new Module(moduleId, moduleContent, isEntry)
            modules.add(module)

            moduleMap.properties.forEach((property) => {
                if (!j.ObjectProperty.check(property)) return
                if (!j.StringLiteral.check(property.key)) return
                if (!j.NumericLiteral.check(property.value)) return

                const shortName = property.key.value
                const moduleId = property.value.value

                const prevShortName = moduleIdMapping[moduleId]
                if (!!prevShortName && prevShortName !== shortName) {
                    console.warn(`Module ${moduleId} has multiple short names: ${prevShortName} and ${shortName}`)
                }
                moduleIdMapping[moduleId] = shortName
            })
        })
    })

    if (!modules.size) return null
    return { modules, moduleIdMapping }
}
