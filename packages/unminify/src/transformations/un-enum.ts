import { findIIFEs, isIIFE } from '@wakaru/ast-utils'
import { assertScopeExists } from '@wakaru/ast-utils/assert'
import { mergeComments } from '@wakaru/ast-utils/comments'
import { createObjectProperty } from '@wakaru/ast-utils/object'
import { getNodePosition } from '@wakaru/ast-utils/position'
import { findDeclarations } from '@wakaru/ast-utils/scope'
import { wrapAstTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import { fromPaths } from 'jscodeshift/src/Collection'
import type { ASTTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import type { CommentKind } from 'ast-types/lib/gen/kinds'
import type { ASTNode, ASTPath, ArrowFunctionExpression, AssignmentExpression, CallExpression, FunctionExpression, Identifier, JSCodeshift, LogicalExpression, ObjectProperty, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

const iifeArgMatcher: ASTNode = {
    type: 'LogicalExpression',
    operator: '||',
    // @ts-expect-error
    left: { type: 'Identifier' },
    right: {
        type: 'AssignmentExpression',
        operator: '=',
        // @ts-expect-error
        left: { type: 'Identifier' },
        right: {
            type: 'ObjectExpression',
            // @ts-expect-error
            properties: props => props.length === 0,
        },
    },
}

const declArgMatcher: ASTNode = {
    type: 'LogicalExpression',
    operator: '||',
    // @ts-expect-error
    left: { type: 'Identifier' },
    right: {
        type: 'ObjectExpression',
        // @ts-expect-error
        properties: props => props.length === 0,
    },
}

/**
 * Restore TypeScript enum syntax.
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    findIIFEs(j, root)
        .filter((path) => {
            const callee = path.node.callee as FunctionExpression | ArrowFunctionExpression
            if (callee.params.length !== 1) return false

            const args = path.node.arguments
            if (args.length !== 1) return false

            if (!j.match(args[0], iifeArgMatcher)) return false
            const arg = args[0] as LogicalExpression
            const left = arg.left as Identifier
            const right = (arg.right as AssignmentExpression).left as Identifier
            return left.name === right.name
        })
        .forEach((path) => {
            const iifePath = fromPaths([path]).closest(j.ExpressionStatement).get()
            handleEnumIIFE(j, path, iifePath)
        })

    root
        .find(j.VariableDeclaration, {
            declarations: (decls) => {
                if (decls.length !== 1) return false
                const decl = decls[0]
                return j.match(decl, {
                    type: 'VariableDeclarator',
                    // @ts-expect-error
                    id: { type: 'Identifier' },
                    // @ts-expect-error
                    init: init => !j.UnaryExpression.check(init) && isIIFE(j, init),
                })
            },
        })
        .filter((path) => {
            const decl = path.node.declarations[0] as VariableDeclarator
            const init = decl.init as CallExpression
            const callee = init.callee as FunctionExpression | ArrowFunctionExpression
            if (callee.params.length !== 1) return false

            const args = init.arguments
            if (args.length !== 1) return false

            if (!j.match(args[0], declArgMatcher)) return false
            const left = (path.node.declarations[0] as VariableDeclarator).id as Identifier
            const arg = args[0] as LogicalExpression
            const right = arg.left as Identifier
            return left.name === right.name
        })
        .forEach((path) => {
            const callExpr = path.get('declarations', 0, 'init') as ASTPath<CallExpression>
            handleEnumIIFE(j, callExpr, path)
        })
}

function handleEnumIIFE(j: JSCodeshift, path: ASTPath<CallExpression>, iifePath: ASTPath) {
    const isVariableDecl = j.VariableDeclaration.check(iifePath.node)

    const callExpr = path.node as CallExpression

    const arg = callExpr.arguments[0] as LogicalExpression
    const callee = callExpr.callee as FunctionExpression | ArrowFunctionExpression
    const body = callee.body
    if (!j.BlockStatement.check(body)) return

    const internalEnumName = (callee.params[0] as Identifier).name

    const scope = path.scope
    assertScopeExists(scope)

    const externalEnumName = (arg.left as Identifier).name

    // collect all enum properties
    const enumProperties = new Map<ObjectProperty['key'], ObjectProperty['value']>()
    const enumReverseProperties = new Map<ObjectProperty['key'], ObjectProperty['value']>()
    const enumComments = new Map<ObjectProperty['key'], CommentKind[]>()
    for (const statement of body.body) {
        // if the enum decl is wrapped in an variable declaration
        // the last statement is a return statement
        if (
            isVariableDecl
            && j.ReturnStatement.check(statement)
            && j.Identifier.check(statement.argument)
            && statement.argument.name === internalEnumName
        ) {
            continue
        }

        if (!j.ExpressionStatement.check(statement)) return
        const expression = statement.expression
        if (!j.AssignmentExpression.check(expression)) return

        const { left, right } = expression
        if (!j.MemberExpression.check(left)) return
        if (!j.StringLiteral.check(right)) return

        // string enum does not have a reverse mapping
        // enum['KEY'] = VALUE
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
        ) {
            // enum[enum.KEY = VALUE] = 'KEY'
            if (
                left.property.left.computed
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
                continue
            }

            // enum[enum['KEY'] = VALUE] = 'KEY'
            if (
                !left.property.left.computed
                && j.Identifier.check(left.property.left.object)
                && left.property.left.object.name === internalEnumName
                && j.Identifier.check(left.property.left.property)
            ) {
                const leftKey = left.property.left.property
                const leftValue = left.property.right
                const rightKey = leftValue
                const rightValue = right
                if (leftKey.name !== rightValue.value) return

                const leftK = j.identifier(leftKey.name)
                enumProperties.set(leftK, leftValue)
                enumComments.set(leftK, statement.comments ?? [])

                enumReverseProperties.set(rightKey, rightValue)
                if (enumReverseProperties.size === 1) {
                    enumComments.set(rightKey, [j.commentLine(' reverse mapping', true)])
                }
                continue
            }
        }
    }

    if (enumProperties.size === 0) return // bail if we can't find any enum properties

    let declarator: VariableDeclarator | null = null
    const decls = findDeclarations(scope, externalEnumName).closest(j.VariableDeclarator)
    if (isVariableDecl) declarator = (iifePath.node as VariableDeclaration).declarations[0] as VariableDeclarator
    else {
        // find the closest declaration before the enum IIFE
        const candidates = decls
            .filter((path) => {
                return (getNodePosition(path.node)?.start ?? 0) < (getNodePosition(iifePath.node)?.start ?? 0)
            })
            .paths()
        if (candidates.length > 0) declarator = candidates.at(-1)!.node
    }
    if (!declarator) return // bail if we can't find the declaration

    // construct the new enum object
    const enumIdent = j.identifier(externalEnumName)
    const enumObject = j.objectExpression(
        [...enumProperties.entries(), ...enumReverseProperties.entries()]
            .map(([key, value]) => {
                const prop = createObjectProperty(j, key, value)
                prop.computed = !j.Identifier.check(key) && !j.NumericLiteral.check(key)
                const comments = enumComments.get(key)
                if (comments) mergeComments(prop, comments)
                return prop
            }),
    )

    const isFirstEnumDecl = decls.paths().findIndex(path => path.node === declarator) === 0
    const isVariableAssign = isVariableDecl || !declarator.init
    const shouldAddSpread = !isFirstEnumDecl || !isVariableAssign
    if (shouldAddSpread) {
        // ...(VAR || (VAR = {}))
        const spreadExtend = j.spreadElement(
            j.logicalExpression('||', enumIdent, j.objectExpression([])),
        )
        enumObject.properties.unshift(spreadExtend)
    }

    if (isVariableAssign) {
        declarator.init = enumObject
        if (!isVariableDecl) iifePath.prune()
        scope.markAsStale()
    }
    else {
        const enumIdent = j.identifier(externalEnumName)
        const enumExpr = j.expressionStatement(j.assignmentExpression('=', enumIdent, enumObject))
        iifePath.replace(enumExpr)
    }
}

export default wrapAstTransformation(transformAST)
