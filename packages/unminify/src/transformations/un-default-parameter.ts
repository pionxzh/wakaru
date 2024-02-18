import { isLogicalNot, isUndefined } from '@wakaru/ast-utils/matchers'
import { isVariableIdentifier } from '@wakaru/ast-utils/reference'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import { logicalExpressionToConditionalExpression, negateCondition } from '../utils/condition'
import type { PatternKind, StatementKind } from 'ast-types/lib/gen/kinds'
import type { ASTPath, ArrowFunctionExpression, AssignmentExpression, AssignmentPattern, BinaryExpression, BlockStatement, ClassMethod, ConditionalExpression, FunctionDeclaration, FunctionExpression, Identifier, JSCodeshift, MemberExpression, NumericLiteral, ObjectMethod } from 'jscodeshift'

/**
 * Restore parameters and default parameters.
 *
 * Note: To avoid the complexity of matching, we are assuming rule `un-flip-comparisons` is applied before this rule.
 *
 * @example
 * function foo(a, b) {
 *   if (a === void 0) { a = 1; }
 *   ...
 * }
 * ->
 * function foo(a = 1, b) {
 *   ...
 * }
 *
 * @example
 * function add2() {
 *   var a = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : 2;
 *   var b = arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : 3;
 *   return a + b;
 * }
 * ->
 * function add(a = 2, b = 3) {
 *   return a + b;
 * }
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-parameters
 */
export default createJSCodeshiftTransformationRule({
    name: 'un-default-parameter',
    transform: (context) => {
        const { root, j } = context

        root
            .find(j.FunctionDeclaration, {
                body: { type: 'BlockStatement' },
            })
            .forEach((path) => {
                handleBody(j, path)
            })

        // var fn = function (a, b) {}
        // { set fn(a, b) {} }
        root
            .find(j.FunctionExpression, {
                body: { type: 'BlockStatement' },
            })
            .forEach((path) => {
                handleBody(j, path)
            })

        root
            .find(j.ArrowFunctionExpression, {
                body: { type: 'BlockStatement' },
            })
            .forEach((path) => {
                handleBody(j, path)
            })

        root
            .find(j.ObjectMethod, {
                body: { type: 'BlockStatement' },
            })
            .forEach((path) => {
                handleBody(j, path)
            })

        root
            .find(j.ClassMethod, {
                body: { type: 'BlockStatement' },
            })
            .forEach((path) => {
                handleBody(j, path)
            })
    },
})

/**
 * The threshold of the body length.
 *
 * To avoid the performance issue, we only handle the first 15 statements.
 */
const BODY_LENGTH_THRESHOLD = 15

function handleBody(j: JSCodeshift, path: ASTPath<FunctionDeclaration | FunctionExpression | ArrowFunctionExpression | ObjectMethod | ClassMethod>) {
    const body = (path.node.body as BlockStatement).body
    if (body.length === 0) return

    const params = path.node.params

    const bodyInThreshold = body.slice(0, BODY_LENGTH_THRESHOLD)
    const bodyOutOfThreshold = body.slice(BODY_LENGTH_THRESHOLD)

    const filteredBodyInThreshold = bodyInThreshold.filter((statement, index) => {
        /**
         * Loose mode
         *
         * @example
         * if (_ref === void 0) _ref = 1;
         */
        if (
            j.IfStatement.check(statement)
            && j.BinaryExpression.check(statement.test)
            && j.Identifier.check(statement.test.left)
            && statement.test.operator === '==='
            && isUndefined(j, statement.test.right)
        ) {
            const identifier = statement.test.left

            const existingDefaultParam = getExistingDefaultParam(j, params, identifier.name)
            if (existingDefaultParam) return true

            let assignmentExp: AssignmentExpression | undefined
            if (
                j.ExpressionStatement.check(statement.consequent)
                && j.AssignmentExpression.check(statement.consequent.expression)
            ) {
                assignmentExp = statement.consequent.expression
            }

            if (
                j.BlockStatement.check(statement.consequent)
                && statement.consequent.body.length === 1
                && j.ExpressionStatement.check(statement.consequent.body[0])
                && j.AssignmentExpression.check(statement.consequent.body[0].expression)
            ) {
                assignmentExp = statement.consequent.body[0].expression
            }

            if (!assignmentExp) return true

            if (
                assignmentExp.operator === '='
                && j.Identifier.check(assignmentExp.left)
                && assignmentExp.left.name === identifier.name
            ) {
                const { right } = assignmentExp

                const previousStatements = bodyInThreshold.slice(0, index)
                if (previousStatements.some(statement => isIdentifierUsedIn(j, statement, identifier.name))) {
                    return true
                }

                const exitingParam = getExistingParam(j, params, identifier.name)
                if (exitingParam) {
                    params.splice(params.indexOf(exitingParam), 1, j.assignmentPattern(exitingParam, right))
                    return false
                }

                params.push(j.assignmentPattern(identifier, right))
                return false
            }

            return true
        }

        /**
         * @example
         * // Normal parameter
         * var _ref = arguments.length > 2 ? arguments[2] : undefined;
         *
         * @example
         * // Default parameter
         * var _ref = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : 2;
         * var _ref = arguments.length > 2 && arguments[2] !== undefined && arguments[2];
         * var _ref = !(arguments.length > 2) || arguments[2] === undefined || arguments[2];
         */
        if (
            j.VariableDeclaration.check(statement)
         && statement.declarations.length === 1
         && j.VariableDeclarator.check(statement.declarations[0])
         && j.Identifier.check((statement.declarations[0]).id)
         && (
             j.ConditionalExpression.check((statement.declarations[0]).init)
          || j.LogicalExpression.check((statement.declarations[0]).init)
         )
        ) {
            const declarator = statement.declarations[0]
            const identifier = declarator.id as Identifier
            if (getExistingDefaultParam(j, params, identifier.name)) return true

            const _init = (statement.declarations[0]).init
            let init = j.LogicalExpression.check(_init)
                ? logicalExpressionToConditionalExpression(j, _init)
                : _init

            if (
                j.LogicalExpression.check(init.test)
             && isLogicalNot(j, init.test.left)
            ) {
                init = negateCondition(j, init) as ConditionalExpression
            }

            const normalMatch = matchNormalParameter(j, init)
            if (normalMatch) {
                const exitingParam = getExistingParam(j, params, identifier.name)
                if (exitingParam) {
                    params.splice(params.indexOf(identifier), 1, identifier)
                    return false
                }

                params.splice(normalMatch.index, 0, identifier)
                return false
            }

            const defaultMatch = matchDefaultParameter(j, init)
            if (defaultMatch) {
                const defaultParam = init.alternate

                const exitingParam = getExistingParam(j, params, identifier.name)
                if (exitingParam) {
                    params.splice(params.indexOf(identifier), 1, j.assignmentPattern(identifier, defaultParam))
                    return false
                }

                params.splice(defaultMatch.index, 0, j.assignmentPattern(identifier, defaultParam))
                return false
            }
        }

        // TODO: rest parameter

        return true
    });

    (path.node.body as BlockStatement).body = [...filteredBodyInThreshold, ...bodyOutOfThreshold]
}

function isIdentifierUsedIn(j: JSCodeshift, statement: StatementKind, identifierName: string) {
    return j(statement)
        .find(j.Identifier, { name: identifierName })
        // FIXME: should use findReferences
        .some(path => isVariableIdentifier(j, path))
}

function getExistingParam(j: JSCodeshift, params: PatternKind[], identifierName: string) {
    return params.find((param): param is Identifier => {
        return j.Identifier.check(param)
            && param.name === identifierName
    })
}

function getExistingDefaultParam(j: JSCodeshift, params: PatternKind[], identifierName: string) {
    return params.find((param): param is AssignmentPattern => {
        return j.AssignmentPattern.check(param)
            && j.Identifier.check(param.left)
            && param.left.name === identifierName
    })
}

/**
 * arguments.length > 1 ? arguments[1] : undefined;
 */
function matchNormalParameter(j: JSCodeshift, node: ConditionalExpression) {
    const isMatch = j.match(node, {
        type: 'ConditionalExpression',
        test: {
            type: 'BinaryExpression',
            left: {
                object: {
                    // @ts-expect-error
                    type: 'Identifier',
                    name: 'arguments',
                },
                property: {
                    // @ts-expect-error
                    type: 'Identifier',
                    name: 'length',
                },
            },
            operator: '>',
            right: {
                type: 'NumericLiteral',
                // @ts-expect-error
                value: (value: number) => value >= 0,
            },
        },
        consequent: {
            type: 'MemberExpression',
            object: {
                type: 'Identifier',
                name: 'arguments',
            },
            property: {
                type: 'NumericLiteral',
                // @ts-expect-error
                value: (value: number) => value >= 0,
            },
            computed: true,
        },
        // @ts-expect-error
        alternate: alternate => isUndefined(j, alternate),
    })
    if (!isMatch) return false

    const index1 = ((node.test as BinaryExpression).right as NumericLiteral).value
    const index2 = ((node.consequent as MemberExpression).property as NumericLiteral).value
    if (index1 !== index2) return false

    return {
        index: index1,
    }
}

/**
 * arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : ...;
 */
function matchDefaultParameter(j: JSCodeshift, node: ConditionalExpression) {
    const isMatch = j.match(node, {
        type: 'ConditionalExpression',
        test: {
            type: 'LogicalExpression',
            left: {
                type: 'BinaryExpression',
                left: {
                    object: {
                        // @ts-expect-error
                        type: 'Identifier',
                        name: 'arguments',
                    },
                    property: {
                        // @ts-expect-error
                        type: 'Identifier',
                        name: 'length',
                    },
                },
                operator: '>',
                right: {
                    type: 'NumericLiteral',
                    // @ts-expect-error
                    value: (value: unknown) => value >= 0,
                },
            },
            operator: '&&',
            right: {
                type: 'BinaryExpression',
                left: {
                    type: 'MemberExpression',
                    object: {
                        type: 'Identifier',
                        name: 'arguments',
                    },
                    property: {
                        type: 'NumericLiteral',
                        // @ts-expect-error
                        value: (value: number) => value >= 0,
                    },
                    computed: true,
                },
                operator: '!==',
                // @ts-expect-error
                right: (right: unknown) => isUndefined(j, right),
            },
        },
        consequent: {
            type: 'MemberExpression',
            object: {
                type: 'Identifier',
                name: 'arguments',
            },
            property: {
                type: 'NumericLiteral',
                // @ts-expect-error
                value: (value: number) => value >= 0,
            },
            computed: true,
        },
        // @ts-expect-error
        alternate: alternate => !isUndefined(j, alternate),
    })
    if (!isMatch) return false

    const index1 = (((node.test as BinaryExpression).left as BinaryExpression).right as NumericLiteral).value
    const index2 = ((((node.test as BinaryExpression).right as BinaryExpression).left as MemberExpression).property as NumericLiteral).value
    const index3 = ((node.consequent as MemberExpression).property as NumericLiteral).value
    if (index1 !== index2 || index1 !== index3 || index2 !== index3) return false

    return {
        index: index1,
    }
}
