import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { Literal, MemberExpression } from 'jscodeshift'

/**
 * Restore template literal syntax from string concatenation.
 *
 * @example
 * // TypeScript / Babel / SWC / esbuild
 * "the ".concat(first, " take the ").concat(second, " and ").concat(third);
 * ->
 * `the ${first} take the ${second} and ${third}`
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'Literal',
                    value: (v: any) => typeof v === 'string',
                },
                property: {
                    type: 'Identifier',
                    name: 'concat',
                },
            },
        })
        .forEach((path) => {
            const object = (path.node.callee as MemberExpression).object as Literal

            // goes up the tree to find the parent CallExpression and check if it's a concat
            // this is to find the start of the concat chain
            // and collect all arguments
            let parent = path
            const args = [object]
            while (parent) {
                // @ts-expect-error skip check for object and property
                args.push(...parent.node.arguments)
                if (j.match(parent?.parent?.parent, {
                    type: 'CallExpression',
                    callee: {
                        type: 'MemberExpression',
                        object: {
                            type: 'CallExpression',
                            // @ts-expect-error skip check for object and property
                            callee: {
                                type: 'MemberExpression',
                            },
                        },
                        property: {
                            type: 'Identifier',
                            name: 'concat',
                        },
                    },
                })) {
                    parent = parent.parent.parent
                    continue
                }

                break
            }

            if (!j.CallExpression.check(parent.node)) return

            const templateLiteral = args.reduce((acc, arg) => {
                if (j.Literal.check(arg)) return acc + arg.value

                return `${acc}\${${j(arg).toSource()}}`
            }, '')

            j(parent).replaceWith(j.templateLiteral([j.templateElement({ raw: templateLiteral, cooked: templateLiteral }, true)], []))
        })
}

export default wrap(transformAST)
