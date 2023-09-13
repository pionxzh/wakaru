import { addImportSpecifier, findImportWithDefaultSpecifier } from '../utils/import'
import { insertBefore } from '../utils/insert'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
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

            const importDecl = findImportWithDefaultSpecifier(j, root, object.name)
            if (importDecl) {
                addImportSpecifier(j, importDecl, property.name)

                const newCallExpression = j.callExpression(j.identifier(property.name), node.arguments)
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
}

export default wrap(transformAST)
