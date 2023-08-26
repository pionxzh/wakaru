import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ASTPath, CallExpression, JSCodeshift, MemberExpression } from 'jscodeshift'

/**
 * Convert function calls on instances of built-in objects to equivalent calls on their prototypes.
 *
 * Rule `unsafe_proto` will convert the following code:
 * ```js
 * Array.prototype.splice.apply(a, [1, 2, b, c]);
 * Function.prototype.call.apply(console.log, console, [ "foo" ]);
 * Number.prototype.toFixed.call(Math.PI, 2);
 * Object.prototype.hasOwnProperty.call(d, "foo");
 * RegExp.prototype.test.call(/foo/, "bar");
 * String.prototype.indexOf.call(e, "bar");
 * ```
 * Into:
 * ```js
 * [].splice.apply(a, [1, 2, b, c]);
 * (function() {}).call.apply(console.log, console, [ "foo" ]);
 * 0..toFixed.call(Math.PI, 2);
 * ({}).hasOwnProperty.call(d, "foo");
 * /t/.test.call(/foo/, "bar");
 * "".indexOf.call(e, "bar");
 * ```
 *
 * And we will convert them back
 *
 * @see Terser: `unsafe_proto`
 * @see https://github.com/terser/terser/blob/27c0a3b47b429c605e2243df86044fc00815060f/test/compress/properties.js#L597
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    // Array
    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'MemberExpression',
                    object: {
                        type: 'ArrayExpression',
                        elements: (node: any) => node.length === 0,
                    },
                    property: {
                        type: 'Identifier',
                        name: (name: string) => name in Array.prototype,
                    },
                },
                property: {
                    type: 'Identifier',
                    name: (name: string) => name === 'call' || name === 'apply',
                },
            },
        })
        .forEach(path => replaceWithPrototype(j, path, 'Array'))

    // Number
    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'MemberExpression',
                    object: {
                        type: 'Literal',
                        value: 0,
                    },
                    property: {
                        type: 'Identifier',
                        name: (name: string) => name in Number.prototype,
                    },
                },
                property: {
                    type: 'Identifier',
                    name: (name: string) => name === 'call' || name === 'apply',
                },
            },
        })
        .forEach(path => replaceWithPrototype(j, path, 'Number'))

    // Object
    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'MemberExpression',
                    object: {
                        type: 'ObjectExpression',
                        properties: (node: any) => node.length === 0,
                    },
                    property: {
                        type: 'Identifier',
                        name: (name: string) => name in Object.prototype,
                    },
                },
                property: {
                    type: 'Identifier',
                    name: (name: string) => name === 'call' || name === 'apply',
                },
            },
        })
        .forEach(path => replaceWithPrototype(j, path, 'Object'))

    // RegExp
    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'MemberExpression',
                    object: {
                        type: 'Literal',
                        regex: {
                            pattern: (pattern: string) => pattern.length > 0,
                        },
                    },
                    property: {
                        type: 'Identifier',
                        name: (name: string) => name in RegExp.prototype,
                    },
                },
                property: {
                    type: 'Identifier',
                    name: (name: string) => name === 'call' || name === 'apply',
                },
            },
        })
        .forEach(path => replaceWithPrototype(j, path, 'RegExp'))

    // String
    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'MemberExpression',
                    object: {
                        type: 'Literal',
                        value: '',
                    },
                    property: {
                        type: 'Identifier',
                        name: (name: string) => name in String.prototype,
                    },
                },
                property: {
                    type: 'Identifier',
                    name: (name: string) => name === 'call' || name === 'apply',
                },
            },
        })
        .forEach((path) => {
            replaceWithPrototype(j, path, 'String')
        })

    // Function
    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'MemberExpression',
                    object: {
                        type: (type: string) => type === 'FunctionExpression' || type === 'ArrowFunctionExpression',
                    },
                    property: {
                        type: 'Identifier',
                        name: (name: string) => name in Function.prototype,
                    },
                },
                property: {
                    type: 'Identifier',
                    name: (name: string) => name === 'call' || name === 'apply',
                },
            },
        })
        .forEach(path => replaceWithPrototype(j, path, 'Function'))
}

function replaceWithPrototype(
    j: JSCodeshift,
    path: ASTPath<CallExpression>,
    prototype: string,
) {
    const callee = path.node.callee as MemberExpression
    const object = callee.object as MemberExpression

    path.replace(
        j.callExpression(
            j.memberExpression(
                j.memberExpression(
                    j.memberExpression(
                        j.identifier(prototype),
                        j.identifier('prototype'),
                    ),
                    object.property,
                ), callee.property,
            ),
            path.node.arguments,
        ),
    )
}

export default wrap(transformAST)
