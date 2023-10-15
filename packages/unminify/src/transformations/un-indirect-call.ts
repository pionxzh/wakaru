import { ImportManager, isTopLevel } from '@wakaru/ast-utils'
import { generateName } from '../utils/identifier'
import { insertAfter } from '../utils/insert'
import { removeDefaultImportIfUnused } from '../utils/scope'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTNode, Identifier, MemberExpression, ObjectPattern, ObjectProperty, SequenceExpression, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

/**
 * Converts indirect call expressions to direct call expressions.
 *
 * @example
 * import s from 'react'
 * (0, s.useRef)(0);
 * ->
 * import { useRef } from 'react'
 * useRef(0);
 *
 * @example
 * const s = require('react')
 * (0, s.useRef)(0);
 * ->
 * const s = require('react')
 * const { useRef } = s
 * useRef(0);
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const importManager = new ImportManager()
    importManager.collectEsModuleImport(j, root)

    /**
     * Adding imports one by one will cause scope issues.
     * So we need to collect all the imports first, then add them all at once.
     */

    /**
     * `s.foo` (indirect call) -> `foo$0` (local specifiers)
     */
    const replaceMapping = new Map<string, string>()

    root
        .find(j.CallExpression, {
            callee: {
                type: 'SequenceExpression',
                expressions: [
                    {
                        type: 'NumericLiteral',
                        value: 0,
                    },
                    {
                        type: 'MemberExpression',
                        object: {
                            type: 'Identifier',
                        },
                        property: {
                            type: 'Identifier',
                        },
                    },
                ],
            },
        })
        .forEach((path) => {
            const { node } = path
            const callee = node.callee as SequenceExpression
            const memberExpression = callee.expressions[1] as MemberExpression
            const object = memberExpression.object as Identifier
            const property = memberExpression.property as Identifier

            /**
             * 1. find `import s from 'react'`
             * 2. check if `useRef` is already imported from the module
             * 3. if not, check if `useRef` is already declared
             * 4. if not, add `import { useRef } from 'react'`
             * 5. else, add `import { useRef as useRef$1 } from 'react'`
             * 6. replace `(0, s.useRef)(0)` with `useRef(0)`
             */

            const defaultSpecifierName = object.name
            const namedSpecifierName = property.name
            const key = `${defaultSpecifierName}.${namedSpecifierName}`

            if (replaceMapping.has(key)) {
                const localName = replaceMapping.get(key)!
                const newCallExpression = j.callExpression(j.identifier(localName), node.arguments)
                path.replace(newCallExpression)
                return
            }

            const defaultImport = importManager.getDefaultImport(defaultSpecifierName)
            if (defaultImport) {
                const source = defaultImport[0]
                const namedImportLocalName = [...(importManager.namedImports.get(source)?.get(namedSpecifierName) ?? [])][0]
                if (namedImportLocalName) {
                    replaceMapping.set(key, namedImportLocalName)
                    const newCallExpression = j.callExpression(j.identifier(namedImportLocalName), node.arguments)
                    path.replace(newCallExpression)
                    return
                }

                const namedSpecifierLocalName = generateName(namedSpecifierName, rootScope, importManager.getAllLocals())
                importManager.addNamedImport(source, namedSpecifierName, namedSpecifierLocalName)
                replaceMapping.set(key, namedSpecifierLocalName)

                const newCallExpression = j.callExpression(j.identifier(namedSpecifierLocalName), node.arguments)
                path.replace(newCallExpression)
                return
            }

            // const s = require('react')
            const requireDecl = root.find(j.VariableDeclaration, {
                declarations: (declarations) => {
                    return declarations.some((d) => {
                        return j.VariableDeclarator.check(d)
                        && j.Identifier.check(d.id) && d.id.name === defaultSpecifierName
                        && j.CallExpression.check(d.init) && j.Identifier.check(d.init.callee) && d.init.callee.name === 'require'
                        && d.init.arguments.length === 1 && (j.StringLiteral.check(d.init.arguments[0]) || j.NumericLiteral.check(d.init.arguments[0]))
                    })
                },
            }).filter(path => isTopLevel(j, path))
            if (requireDecl.size() > 0) {
                // find `const { useRef } = react` or `const { useRef: useRef$0 } = react`
                const propertyDecl = root.find(j.VariableDeclarator, {
                    id: {
                        type: 'ObjectPattern',
                        properties: (properties: ObjectPattern['properties']) => {
                            return properties.some((p) => {
                                return j.ObjectProperty.check(p)
                                && j.Identifier.check(p.key) && p.key.name === property.name
                                && j.Identifier.check(p.value)
                            })
                        },
                    },
                    init: {
                        type: 'Identifier',
                        name: object.name,
                    },
                }).filter((p) => {
                    return isTopLevel(j, p.parent)
                    && isPositionBetween(p.parent.node, requireDecl.get().node, path.node)
                })

                if (propertyDecl.size() === 0) {
                    // generate `const { useRef: useRef$0 } = react`
                    const key = j.identifier(property.name)
                    const valueName = generateName(property.name, rootScope, [...replaceMapping.values()])
                    replaceMapping.set(`${defaultSpecifierName}.${namedSpecifierName}`, valueName)

                    const value = j.identifier(valueName)
                    const objectProperty = j.objectProperty(key, value)
                    objectProperty.shorthand = key.name === value.name

                    // find existing `const { ... } = react`
                    const existingDestructuring = root
                        .find(j.VariableDeclaration, {
                            kind: 'const',
                            declarations: (declarations) => {
                                return declarations.some((d) => {
                                    return j.VariableDeclarator.check(d)
                                    && j.ObjectPattern.check(d.id)
                                    && j.Identifier.check(d.init)
                                    && d.init.name === object.name
                                })
                            },
                        })
                        .filter((p) => {
                            return isTopLevel(j, p)
                            && isPositionBetween(p.node, requireDecl.get().node, path.node)
                        })

                    if (existingDestructuring.size() > 0) {
                        const existingDestructuringNode = existingDestructuring.get().node as VariableDeclaration
                        const objectPattern = existingDestructuringNode.declarations.find((d): d is VariableDeclarator => {
                            return j.VariableDeclarator.check(d)
                            && j.ObjectPattern.check(d.id)
                            && j.Identifier.check(d.init)
                            && d.init.name === object.name
                        })!.id as ObjectPattern
                        objectPattern.properties.push(objectProperty)
                    }
                    else {
                        const variableDeclarator = j.variableDeclarator(
                            j.objectPattern([objectProperty]),
                            j.identifier(object.name),
                        )
                        const variableDeclaration = j.variableDeclaration('const', [variableDeclarator])
                        const requireDeclPath = requireDecl.get()
                        insertAfter(j, requireDeclPath, variableDeclaration)
                    }

                    const newCallExpression = j.callExpression(j.identifier(valueName), node.arguments)
                    path.replace(newCallExpression)
                    rootScope.markAsStale()

                    return
                }

                // extract `useRef$0` from `const { useRef: useRef$0 } = react`
                const propertyNode = propertyDecl.get().node
                const propertyValue = propertyNode.id as ObjectPattern
                const targetProperty = propertyValue.properties.find((p) => {
                    return j.ObjectProperty.check(p) && j.Identifier.check(p.key) && p.key.name === property.name
                }) as ObjectProperty | undefined
                if (!targetProperty) return

                const targetPropertyValue = targetProperty.value as Identifier
                const targetPropertyLocalName = targetPropertyValue.name
                replaceMapping.set(`${defaultSpecifierName}.${namedSpecifierName}`, targetPropertyLocalName)

                const newCallExpression = j.callExpression(j.identifier(targetPropertyLocalName), node.arguments)
                path.replace(newCallExpression)
            }
        })

    importManager.applyImportToRoot(j, root)

    importManager.defaultImports.forEach((defaultImport) => {
        defaultImport.forEach((specifier) => {
            removeDefaultImportIfUnused(j, root, specifier)
        })
    })
}

function isPositionBetween(node: ASTNode, before: ASTNode, after: ASTNode) {
    if (
        'start' in node && typeof node.start === 'number'
        && 'end' in node && typeof node.end === 'number'
        && 'end' in before && typeof before.end === 'number'
        && 'start' in after && typeof after.start === 'number'
    ) {
        return node.start > before.end && node.end < after.start
    }

    // no position info, means it's our newly inserted node
    // assume we have inserted it correctly before
    return true
}

export default wrap(transformAST)
