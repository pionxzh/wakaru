import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
  * @TODO: extends
  * @TODO: rename the remaining old constructor name
  *
  * var Foo = (function() {
  *   function t(name) {
  *     this.name = name;
  *     this.age = 18;
  *   }
  *   t.prototype.logger = function logger() {
  *     console.log("Hello", this.name);
  *   };
  *   t.staticMethod = function staticMethod() {
  *   return t;
  * })();
  *
  * ->
  *
  * class Foo {
  *   constructor() {
  *     this.name = 'bar'
  *     this.age = 18
  *   }
  *   logger() {
  *     console.log("Hello", this.name);
  *   }
  *   static staticMethod() {
  *     console.log('static method')
  *   }
  * }
  *
  * Actually, this is for TypeScript ES6 class transpile result
  * This transformation won't being able to handle the result from babel
  *
  * Just put it here for reference.
  * @see https://babeljs.io/docs/en/babel-plugin-transform-classes
  */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.VariableDeclaration, {
            declarations: [
                {
                    id: {
                        type: 'Identifier',
                    },
                    init: {
                        type: 'CallExpression',
                        callee: {
                            type: (node: any) => node === 'FunctionExpression' || node === 'ArrowFunctionExpression',
                        },
                    },
                },
            ],
        })
        .forEach((p) => {
            const { declarations } = p.node
            if (!declarations) return
            const decl = declarations[0]
            if (!decl || !j.VariableDeclarator.check(decl)) return
            const { id, init } = decl
            if (!id || !init || !j.Identifier.check(id) || !j.CallExpression.check(init)) return
            const { name } = id
            if (!name) return
            const { callee } = init
            if (!(j.FunctionExpression.check(callee) || j.ArrowFunctionExpression.check(callee))) return
            const { body } = callee
            if (!body || !j.BlockStatement.check(body)) return
            const { body: bodyBody } = body
            if (!bodyBody) return

            const bodyList: any[] = []

            let internalName = ''
            bodyBody.forEach((bodyNode) => {
                // drop the last return statement
                if (j.ReturnStatement.check(bodyNode)) return

                // constructor
                if (j.FunctionDeclaration.check(bodyNode)) {
                    const { id, params, body } = bodyNode
                    if (!id || !params || !body) return
                    const { name } = id
                    if (!name) return
                    internalName = name
                    const { body: bodyBody } = body
                    if (!bodyBody || bodyBody.length === 0) return
                    const constructor = j.classMethod(
                        'constructor',
                        j.identifier('constructor'),
                        params,
                        body,
                    )
                    bodyList.push(constructor)
                    return
                }

                if (j.ExpressionStatement.check(bodyNode) && j.AssignmentExpression.check(bodyNode.expression)) {
                    const { left, right } = bodyNode.expression
                    if (!left || !right
                        || !j.MemberExpression.check(left)
                        || !j.Identifier.check(left.property)) return

                    const methodName = left.property.name

                    const isPrototypeMethod = j(left).find(j.MemberExpression, {
                        object: {
                            type: 'Identifier',
                            name: internalName,
                        },
                        property: {
                            type: 'Identifier',
                            name: 'prototype',
                        },
                    }).size() > 0

                    const isStatic = left.object.type === 'Identifier'
                        && left.object.name === internalName
                        && left.property.type === 'Identifier'
                        && left.property.name !== 'prototype'

                    if (j.FunctionExpression.check(right)) {
                        // prototype method -> class method
                        // t.prototype.logger = function logger()...
                        if (isPrototypeMethod) {
                            const { params, body } = right
                            const classMethod = j.classMethod(
                                'method',
                                j.identifier(methodName),
                                params,
                                body,
                            )
                            bodyList.push(classMethod)
                        }

                        // static method
                        else if (isStatic) {
                            const { params, body } = right
                            const staticMethod = j.classMethod(
                                'method',
                                j.identifier(methodName),
                                params,
                                j.blockStatement(body.body),
                                false,
                                true,
                            )
                            bodyList.push(staticMethod)
                        }
                    }
                    else if (isStatic) {
                        // static property
                        const staticProperty = j.classProperty(
                            j.identifier(methodName),
                            right,
                            null,
                            true,
                        )
                        bodyList.push(staticProperty)
                    }
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
                        const classMethod = j.classMethod(
                            'get',
                            j.identifier(propName),
                            [],
                            j.blockStatement(value.body.body),
                        )
                        bodyList.push(classMethod)
                    }

                    if (setterFn.size() > 0) {
                        const { value } = setterFn.nodes()[0]
                        if (!value || !j.FunctionExpression.check(value)) return
                        const classMethod = j.classMethod(
                            'set',
                            j.identifier(propName),
                            value.params,
                            j.blockStatement(value.body.body),
                        )
                        bodyList.push(classMethod)
                    }
                }
            })

            const classBody = j.classBody(bodyList)
            const classDeclaration = j.classDeclaration(
                j.identifier(name),
                classBody,
            )
            p.replace(classDeclaration)
        })
}

export default wrap(transformAST)
