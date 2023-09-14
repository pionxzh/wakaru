import { generateName } from '../utils/identifier'
import { addImportSpecifier, findImportFromSource, findImportWithDefaultSpecifier, findImportWithNamedSpecifier } from '../utils/import'
import { insertBefore } from '../utils/insert'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { Identifier, MemberExpression, ObjectPattern, SequenceExpression } from 'jscodeshift'

interface Params {
    unsafe?: boolean
}

/**
 * Converts indirect call expressions to direct call expressions.
 *
 * @example
 * import s from 'react'
 * (0, s.useRef)(0);
 * ->
 * import { useRef } from 'react'
 * useRef(0);
 */
export const transformAST: ASTTransformation<Params> = (context, params = { unsafe: false }) => {
    const { root, j } = context

    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    /**
     * Adding imports one by one will cause scope issues.
     * So we need to collect all the imports first, then add them all at once.
     */

    // `s.foo` (indirect call) -> `foo$0` (local specifiers)
    const replaceMapping = new Map<string, string>()
    // `foo$0` (local specifier) -> `foo` (imported specifier)
    const specifierMapping = new Map<string, string>()
    // `foo$0` (local specifier) -> `module` (module name)
    const moduleMapping = new Map<string, string>()

    root
        .find(j.CallExpression, {
            callee: {
                type: 'SequenceExpression',
                expressions: [
                    {
                        type: 'Literal',
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

            const importDecl = findImportWithDefaultSpecifier(j, rootScope, defaultSpecifierName)
            if (importDecl) {
                const source = importDecl.source.value
                if (typeof source !== 'string') return
                const namedImportSpecifierPath = findImportWithNamedSpecifier(j, rootScope, namedSpecifierName, source)
                if (namedImportSpecifierPath) {
                    // @ts-expect-error
                    const localName = namedImportSpecifierPath.node.local?.name.value ?? namedSpecifierName
                    replaceMapping.set(key, localName)
                    specifierMapping.set(localName, namedSpecifierName)
                    moduleMapping.set(localName, source)

                    const newCallExpression = j.callExpression(j.identifier(localName), node.arguments)
                    path.replace(newCallExpression)
                    return
                }

                const namedSpecifierLocalName = generateName(namedSpecifierName, rootScope, [...replaceMapping.values()])
                replaceMapping.set(key, namedSpecifierLocalName)
                specifierMapping.set(namedSpecifierLocalName, namedSpecifierName)
                moduleMapping.set(namedSpecifierLocalName, source)

                const newCallExpression = j.callExpression(j.identifier(namedSpecifierLocalName), node.arguments)
                path.replace(newCallExpression)
                return
            }

            if (params.unsafe) {
                // find `const { useRef } = react`
                const propertyDecl = root.find(j.VariableDeclarator, {
                    id: {
                        type: 'ObjectPattern',
                        properties: (properties: ObjectPattern['properties']) => {
                            return properties.some((p) => {
                                return j.Property.check(p)
                                && j.Identifier.check(p.key) && p.key.name === property.name
                                && j.Identifier.check(p.value) && p.value.name === property.name
                            })
                        },
                    },
                    init: {
                        type: 'Identifier',
                        name: object.name,
                    },
                })
                if (propertyDecl.size() === 0) {
                    // const { useRef } = react
                    const id = j.identifier(property.name)
                    const objectProperty = j.objectProperty(id, id)
                    objectProperty.shorthand = true
                    const variableDeclaration = j.variableDeclaration(
                        'const',
                        [j.variableDeclarator(
                            j.objectPattern([objectProperty]),
                            j.identifier(object.name),
                        )],
                    )

                    insertBefore(j, path, variableDeclaration)
                }

                const newCallExpression = j.callExpression(j.identifier(property.name), node.arguments)
                path.replace(newCallExpression)
            }
        })

    specifierMapping.forEach((importedName, localName) => {
        const moduleName = moduleMapping.get(localName)
        if (!moduleName) return

        const importDecl = findImportFromSource(j, root, moduleName)
        if (!importDecl) return

        addImportSpecifier(j, importDecl, importedName, localName)
    })
}

export default wrap(transformAST)
