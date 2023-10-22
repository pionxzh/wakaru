import { isTopLevel } from '@wakaru/ast-utils'
import { assertScopeExists } from '../utils/assert'
import { mergeComments } from '../utils/comments'
import { findDeclaration } from '../utils/scope'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { CommentKind } from 'ast-types/lib/gen/kinds'
import type { ASTPath, ArrowFunctionExpression, AssignmentExpression, CallExpression, FunctionExpression, Identifier, LogicalExpression, ObjectProperty, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

/**
 * Restore TypeScript enum syntax.
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.ExpressionStatement, {
            expression: {
                type: 'CallExpression',
                callee: {
                    type: (type: string) => {
                        return type === 'FunctionExpression'
                        || type === 'ArrowFunctionExpression'
                    },
                    params: params => params.length === 1 && j.Identifier.check(params[0]),
                },
                arguments: [
                    {
                        type: 'LogicalExpression',
                        operator: '||',
                        left: {
                            type: 'Identifier',
                        },
                        right: {
                            type: 'AssignmentExpression',
                            operator: '=',
                            left: {
                                type: 'Identifier',
                            },
                            right: {
                                type: 'ObjectExpression',
                                properties: [],
                            },
                        },
                    },
                ],
            },
        })
        .forEach((path) => {
            if (!isTopLevel(j, path)) return

            const expression = path.node.expression as CallExpression
            if (expression.arguments.length !== 1) return

            const arg = expression.arguments[0] as LogicalExpression
            const callee = expression.callee as FunctionExpression | ArrowFunctionExpression
            const body = callee.body
            if (!j.BlockStatement.check(body)) return

            const internalEnumName = (callee.params[0] as Identifier).name

            const scope = path.scope
            assertScopeExists(scope)

            const externalEnumName = (arg.left as Identifier).name
            const rightIdent = (arg.right as AssignmentExpression).left as Identifier
            if (externalEnumName !== rightIdent.name) return // bail if the shape is not `VAR || (VAR = {})`

            const ident = findDeclaration(scope, externalEnumName)
            if (!ident) return // bail if we can't find the declaration
            const declarator = ident.parent as ASTPath<VariableDeclarator>
            if (!j.VariableDeclarator.check(declarator.node)) return
            const declaration = declarator.parent as ASTPath<VariableDeclaration>
            if (!j.VariableDeclaration.check(declaration.node)) return

            // collect all enum properties
            const enumProperties = new Map<ObjectProperty['key'], ObjectProperty['value']>()
            const enumReverseProperties = new Map<ObjectProperty['key'], ObjectProperty['value']>()
            const enumComments = new Map<ObjectProperty['key'], CommentKind[]>()
            for (const statement of body.body) {
                if (!j.ExpressionStatement.check(statement)) return
                const expression = statement.expression
                if (!j.AssignmentExpression.check(expression)) return

                const { left, right } = expression
                if (!j.MemberExpression.check(left)) return
                if (!j.StringLiteral.check(right)) return

                // string enum does not have a reverse mapping
                if (left.computed && j.StringLiteral.check(left.property)) {
                    const key = j.identifier(left.property.value)
                    enumProperties.set(key, right)
                    enumComments.set(key, statement.comments ?? [])
                    continue
                }
                if (!left.computed && j.Identifier.check(left.property)) {
                    const key = left.property
                    enumProperties.set(key, right)
                    enumComments.set(key, statement.comments ?? [])
                    continue
                }

                // non-string enum
                if (
                    left.computed
                    && j.AssignmentExpression.check(left.property)
                    && j.MemberExpression.check(left.property.left)
                    && left.property.left.computed
                    && j.Identifier.check(left.property.left.object)
                    && left.property.left.object.name === internalEnumName
                    && j.StringLiteral.check(left.property.left.property)
                ) {
                    const leftKey = left.property.left.property
                    const leftValue = left.property.right
                    const rightKey = leftValue
                    const rightValue = right
                    if (leftKey.value !== rightValue.value) return

                    const leftK = j.identifier(leftKey.value)
                    enumProperties.set(leftK, leftValue)
                    enumComments.set(leftK, statement.comments ?? [])

                    enumReverseProperties.set(rightKey, rightValue)
                    if (enumReverseProperties.size === 1) {
                        enumComments.set(rightKey, [j.commentLine(' reverse mapping', true)])
                    }
                }
            }

            // construct the new enum object
            const enumObject = j.objectExpression(
                [...enumProperties.entries(), ...enumReverseProperties.entries()]
                    .map(([key, value]) => {
                        const prop = j.objectProperty(key, value)
                        prop.computed = !j.Identifier.check(key)
                        && !j.NumericLiteral.check(key)
                        const comments = enumComments.get(key)
                        if (comments) mergeComments(prop, comments)
                        return prop
                    }),
            )

            if (declarator.node.init) {
                // ...(VAR || (VAR = {}))
                const enumIdent = j.identifier(externalEnumName)
                const spreadExtend = j.spreadElement(
                    j.logicalExpression(
                        '||',
                        enumIdent,
                        j.assignmentExpression('=', enumIdent, j.objectExpression([])),
                    ),
                )
                enumObject.properties.unshift(spreadExtend)
                const enumExpr = j.expressionStatement(j.assignmentExpression('=', enumIdent, enumObject))
                // markParenthesized(path.node, false)
                path.replace(enumExpr)
            }
            else {
                declarator.node.init = enumObject
                path.prune()
                scope.markAsStale()
            }
        })
}

export default wrap(transformAST)
