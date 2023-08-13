import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { AssignmentExpression, CallExpression, ExpressionStatement, FunctionExpression, Identifier, MemberExpression, VariableDeclarator } from 'jscodeshift'

/**
  * Restore `Class` definition from the constructor and the prototype.
  * Currently, this transformation only supports output from TypeScript.
  *
  * @TODO: extends
  * @TODO: rename the remaining old constructor name
  * @TODO: babel
  *
  * @example
  * var Foo = (function() {
  *   function t(name) {
  *     this.name = name;
  *     this.age = 18;
  *   }
  *   t.prototype.logger = function logger() {
  *     console.log("Hello", this.name);
  *   }
  *   t.staticMethod = function staticMethod() {
  *       console.log('static method')
  *   }
  * })();
  *
  * ->
  *
  * class Foo {
  *   constructor() {
  *     this.name = 'bar'
  *     this.age = 18
  *   }
  *   get message() {
  *     return 'Hello' + this.name
  *   }
  *   logger() {
  *     console.log("Hello", this.name);
  *   }
  *   static staticMethod() {
  *     console.log('static method')
  *   }
  * }
  *
  * TODO: useDefineForClassFields
  * @see https://babeljs.io/docs/babel-plugin-transform-classes
  * @see https://www.typescriptlang.org/play?target=1#code/MYGwhgzhAEBiD29oG8BQ1oDswFsCmAXNBAC4BOAlpgObrRjWFYCuOARnmXXcPJqWWbAS8MgAps+IgKrUAlCjoYSACwoQAdJLzQAvFlx4l0Veo0Md+gIwAOOgF86IeNUbiFaDBl794IPBrO1GIARAASeCDOIQA0Jmqa2nIA3A50pGAkFMDEJJnZALJ4qvAAJmIexj4QfgFBYgDkGVk5+CWlDXJpGM3Z0FQZmMCWWHgA7nCIoQBmiCFd9kA
  */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.VariableDeclaration, {
            declarations: [
                {
                    type: 'VariableDeclarator',
                    id: {
                        type: 'Identifier',
                    },
                    init: {
                        type: 'CallExpression',
                        callee: {
                            type: 'FunctionExpression',
                            body: {
                                type: 'BlockStatement',
                                body: [
                                    {
                                        type: 'FunctionDeclaration',
                                        id: {
                                            type: 'Identifier',
                                        },
                                    },
                                ],
                            },
                        },
                        arguments: args => args.length === 0,
                    },
                },
            ],
        })
        .forEach((p) => {
            const decl = p.node.declarations[0] as unknown as VariableDeclarator
            const className = (decl.id as Identifier).name
            const init = decl.init as CallExpression
            const callee = init.callee as FunctionExpression
            const bodyBody = callee.body.body
            if (bodyBody.length < 2) return

            const lastBodyNode = bodyBody[bodyBody.length - 1]
            if (!j.ReturnStatement.check(lastBodyNode)) return
            if (!j.Identifier.check(lastBodyNode.argument)) return

            const internalName = lastBodyNode.argument.name

            const bodyList: any[] = []

            bodyBody.forEach((bodyNode) => {
                // skip the last return statement
                if (j.ReturnStatement.check(bodyNode)) return

                // constructor
                if (j.FunctionDeclaration.check(bodyNode) && bodyNode.id?.name === internalName) {
                    const { params, body } = bodyNode
                    if (params.length === 0 && body.body.length === 0) {
                        // empty constructor
                        return
                    }

                    const value = j.functionExpression(
                        null,
                        params,
                        body,
                    )
                    const constructor = j.methodDefinition(
                        'constructor',
                        j.identifier('constructor'),
                        value,
                        false,
                    )
                    bodyList.push(constructor)
                    return
                }

                // class instance method
                // TheClass.prototype.method = function () {}
                if (j.match(bodyNode, {
                    type: 'ExpressionStatement',
                    expression: {
                        type: 'AssignmentExpression',
                        operator: '=',
                        left: {
                            type: 'MemberExpression',
                            object: {
                                type: 'MemberExpression',
                                object: {
                                    type: 'Identifier',
                                    name: internalName,
                                },
                                property: {
                                    type: 'Identifier',
                                    name: 'prototype',
                                },
                            },
                            property: {
                                type: 'Identifier' as any,
                            },
                        },
                        right: {
                            type: 'FunctionExpression' as any,
                        },
                    },
                })) {
                    const { left, right } = (bodyNode as ExpressionStatement).expression as AssignmentExpression
                    const methodName = ((left as MemberExpression).property as Identifier).name
                    const classMethod = j.methodDefinition(
                        'method',
                        j.identifier(methodName),
                        right as FunctionExpression,
                        false,
                    )
                    bodyList.push(classMethod)
                }

                // class static method
                // TheClass.method = function () {}
                else if (j.match(bodyNode, {
                    type: 'ExpressionStatement',
                    expression: {
                        type: 'AssignmentExpression',
                        operator: '=',
                        left: {
                            type: 'MemberExpression',
                            object: {
                                type: 'Identifier',
                                name: internalName,
                            },
                            property: {
                                type: 'Identifier' as any,
                            },
                        },
                        right: {
                            type: 'FunctionExpression' as any,
                        },
                    },
                })) {
                    const { left, right } = (bodyNode as ExpressionStatement).expression as AssignmentExpression
                    const methodName = ((left as MemberExpression).property as Identifier).name
                    const staticMethod = j.methodDefinition(
                        'method',
                        j.identifier(methodName),
                        right as FunctionExpression,
                        true,
                    )
                    bodyList.push(staticMethod)
                }

                // getter / setter
                /**
                 * Object.defineProperty(t.prototype, "operationUnitIndex", {
                 *   get: function () {
                 *     return this.activeSelfPlayerId == this.uMan.selfUserId ? 0 : 1;
                 *   },
                 *   enumerable: !0,
                 *   configurable: !0
                 * })
                 */
                if (j.ExpressionStatement.check(bodyNode) && j.CallExpression.check(bodyNode.expression)) {
                    const { arguments: args } = bodyNode.expression
                    if (!args) return

                    const [obj, prop, descriptor] = args
                    if (!obj || !prop || !descriptor
                        || !j.Literal.check(prop)
                        || !j.ObjectExpression.check(descriptor)) return

                    const { value: propName } = prop
                    if (!propName || typeof propName !== 'string') return

                    const getterFn = j(descriptor).find(j.Property, {
                        key: {
                            type: 'Identifier',
                            name: 'get',
                        },
                    })
                    const setterFn = j(descriptor).find(j.Property, {
                        key: {
                            type: 'Identifier',
                            name: 'set',
                        },
                    })

                    if (getterFn.size() > 0) {
                        const { value } = getterFn.nodes()[0]
                        if (!value || !j.FunctionExpression.check(value)) return
                        const classMethod = j.methodDefinition(
                            'get',
                            j.identifier(propName),
                            value,
                            false,
                        )
                        bodyList.push(classMethod)
                    }

                    if (setterFn.size() > 0) {
                        const { value } = setterFn.nodes()[0]
                        if (!value || !j.FunctionExpression.check(value)) return
                        const classMethod = j.methodDefinition(
                            'set',
                            j.identifier(propName),
                            value,
                            false,
                        )
                        bodyList.push(classMethod)
                    }
                }
            })

            const classBody = j.classBody(bodyList)
            const classDeclaration = j.classDeclaration(
                j.identifier(className),
                classBody,
            )
            p.replace(classDeclaration)
        })
}

export default wrap(transformAST)
