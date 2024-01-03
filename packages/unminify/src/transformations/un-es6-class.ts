import { findReferences } from '@wakaru/ast-utils/reference'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import { findHelperLocals, removeHelperImport } from '../utils/import'
import type { ASTTransformation } from '@wakaru/shared/rule'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { Scope } from 'ast-types/lib/scope'
import type { AssignmentExpression, CallExpression, ExpressionStatement, FunctionExpression, Identifier, MemberExpression, VariableDeclarator } from 'jscodeshift'

const inheritsModuleName = '@babel/runtime/helpers/inherits'
const inheritsModuleEsmName = '@babel/runtime/helpers/esm/inherits'

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
export const transformAST: ASTTransformation = (context, params) => {
    const { root, j } = context

    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const inheritsHelpers = findHelperLocals(context, params, inheritsModuleName, inheritsModuleEsmName)

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
                            },
                        },
                        arguments: args => args.length <= 1,
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
            if (init.arguments.length === 0 && !j.FunctionDeclaration.check(bodyBody[0])) return
            if (init.arguments.length === 1 && bodyBody.findIndex(node => j.FunctionDeclaration.check(node)) < 1) return

            const lastBodyNode = bodyBody[bodyBody.length - 1]
            if (!j.ReturnStatement.check(lastBodyNode)) return
            let internalName: string
            if (j.Identifier.check(lastBodyNode.argument)) {
                internalName = lastBodyNode.argument.name
            }
            else if (/* Babel */
                j.CallExpression.check(lastBodyNode.argument)
                && j.Identifier.check(lastBodyNode.argument.callee)
                && lastBodyNode.argument.callee.name === '_createClass'
                && j.Identifier.check(lastBodyNode.argument.arguments[0])
            ) {
                internalName = lastBodyNode.argument.arguments[0].name
            }
            else {
                return
            }

            let superClass: ExpressionKind | null = null

            let prototypeNode: ExpressionKind = {
                type: 'MemberExpression',
                object: {
                    type: 'Identifier',
                    name: internalName,
                },
                property: {
                    type: 'Identifier',
                    name: 'prototype',
                },
            }

            const bodyList: any[] = []

            bodyBody.forEach((bodyNode) => {
                // skip the last return statement
                if (j.ReturnStatement.check(bodyNode)) return

                // prototype assignment in Babel loose mode
                if (
                    j.VariableDeclaration.check(bodyNode)
                    && j.VariableDeclarator.check(bodyNode.declarations[0])
                    && j.Identifier.check(bodyNode.declarations[0].id)
                    && bodyNode.declarations[0].init
                    && j.match(bodyNode.declarations[0].init, {
                        type: 'MemberExpression',
                        object: {
                            type: 'Identifier',
                            name: internalName,
                        },
                        property: {
                            type: 'Identifier',
                            name: 'prototype',
                        },
                    })
                ) {
                    prototypeNode = { type: 'Identifier', name: bodyNode.declarations[0].id.name }
                    return
                }

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
                            object: prototypeNode,
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
                if (
                    j.ExpressionStatement.check(bodyNode)
                    && j.CallExpression.check(bodyNode.expression)
                    && (
                        j.match(bodyNode.expression.callee, {
                            type: 'MemberExpression',
                            object: {
                                type: 'Identifier',
                                name: 'Object',
                            },
                            property: {
                                type: 'Identifier',
                                name: 'defineProperty',
                            },
                        })
                        || j.match(bodyNode.expression.callee, {
                            type: 'Identifier',
                            name: '_defineProperty', // Babel
                        })
                        || j.match(bodyNode.expression.callee, {
                            type: 'Identifier',
                            name: '_define_property', // SWC
                        })
                    )
                ) {
                    const { arguments: args } = bodyNode.expression
                    if (!args) return

                    const [obj, prop, descriptor] = args
                    if (!obj || !prop || !descriptor
                        || !j.StringLiteral.check(prop)
                        || !j.ObjectExpression.check(descriptor)) return

                    const { value: propName } = prop
                    if (!propName) return

                    const getterFn = j(descriptor).find(j.ObjectProperty, {
                        key: {
                            type: 'Identifier',
                            name: 'get',
                        },
                    })
                    const setterFn = j(descriptor).find(j.ObjectProperty, {
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

                // extends
                /**
                 * _inherits(SubClass, SuperClass);
                 */
                if (
                    j.ExpressionStatement.check(bodyNode)
                    && j.CallExpression.check(bodyNode.expression)
                    && j.Identifier.check(bodyNode.expression.callee)
                    && (inheritsHelpers.includes(bodyNode.expression.callee.name)
                        || bodyNode.expression.callee.name === '_inherits' /* Babel/SWC */
                        || bodyNode.expression.callee.name === '_inheritsLoose'
                        || bodyNode.expression.callee.name === '__extends' /* TypeScript */)
                    && bodyNode.expression.arguments.length === 2
                    && j.match(bodyNode.expression.arguments[0], {
                        type: 'Identifier',
                        name: internalName,
                    })
                    && !j.SpreadElement.check(init.arguments[0])
                ) {
                    superClass = init.arguments[0]
                }
            })

            const classBody = j.classBody(bodyList)
            const classDeclaration = j.classDeclaration(
                j.identifier(className),
                classBody,
                superClass,
            )
            p.replace(classDeclaration)
        })

    inheritsHelpers
        .filter(helperLocal => findReferences(j, rootScope, helperLocal).length === 1)
        .forEach((helperLocal) => {
            removeHelperImport(j, rootScope, helperLocal)
        })
}

export default createJSCodeshiftTransformationRule({
    name: 'un-es6-class',
    transform: transformAST,
})
