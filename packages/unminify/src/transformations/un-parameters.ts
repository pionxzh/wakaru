import { isUndefined } from '../utils/checker'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { PatternKind } from 'ast-types/lib/gen/kinds'
import type { ASTPath, AssignmentExpression, AssignmentPattern, ConditionalExpression, FunctionDeclaration, FunctionExpression, Identifier, JSCodeshift } from 'jscodeshift'

/**
 * Restore parameters. Support normal parameters and default parameters.
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
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.FunctionDeclaration, {
            body: {
                type: 'BlockStatement',
            },
        })
        .forEach((path) => {
            handleBody(j, path)
        })

    // var fn = function (a, b) {}
    // { set fn(a, b) {} }
    root
        .find(j.FunctionExpression, {
            body: {
                type: 'BlockStatement',
            },
        })
        .forEach((path) => {
            handleBody(j, path as ASTPath<FunctionExpression>)

            /**
             * FIXME: can be super slow?
             *
             * Hacky fix for setter function's wrong output.
             * See https://github.com/facebook/jscodeshift/issues/567
             */
            if (j.Property.check(path.parentPath.node)) {
                const newProperty = j.property(
                    path.parentPath.node.kind,
                    path.parentPath.node.key,
                    j.functionExpression(
                        path.node.id,
                        path.node.params,
                        path.node.body,
                        path.node.generator,
                        path.node.expression,
                    ),
                )

                path.parentPath.replace(newProperty)
            }
        })
}

/**
 * The threshold of the body length.
 *
 * To avoid the performance issue, we only handle the first 15 statements.
 */
const BODY_LENGTH_THRESHOLD = 15

const normalParameterRE = /arguments\.length\s?>\s?(\d+)\s?\?\s?arguments\[(\d+)\]\s:\s?undefined;?/
const defaultParameterRE = /arguments\.length\s?>\s?(\d+)\s?&&\s?arguments\[(\d+)\]\s?!==\s?undefined\s?\?\s?arguments\[(\d+)\]\s?:.+/

function handleBody(j: JSCodeshift, path: ASTPath<FunctionDeclaration | FunctionExpression>) {
    const body = path.node.body.body
    if (body.length === 0) return

    const params = path.node.params

    const bodyInThreshold = body.slice(0, BODY_LENGTH_THRESHOLD)
    const bodyOutOfThreshold = body.slice(BODY_LENGTH_THRESHOLD)

    const filteredBodyInThreshold = bodyInThreshold.filter((statement) => {
        /**
         * Loose mode
         *
         * @example
         * if (_ref === void 0) _ref = 1;
         */
        if (
            j.IfStatement.check(statement)
            && j.BinaryExpression.check(statement.test)
            && statement.test.operator === '==='
            && isUndefined(j, statement.test.right)
            && j.Identifier.check(statement.test.left)
        ) {
            const identifier = statement.test.left

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
                const { left, right } = assignmentExp

                // TODO: check if the parameter is already used before

                // Check if the parameter is already defined
                const existingDefaultParam = getExistingDefaultParam(j, params, left.name)
                if (existingDefaultParam) return true

                const exitingParam = getExistingParam(j, params, left.name)
                if (exitingParam) {
                    const index = params.indexOf(exitingParam)
                    const newParams = params.slice()
                    newParams.splice(index, 1, j.assignmentPattern(left, right))
                    path.node.params = newParams
                    return false
                }

                params.push(j.assignmentPattern(left, right))
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
         */
        if (
            j.VariableDeclaration.check(statement)
            && statement.declarations.length === 1
            && j.VariableDeclarator.check(statement.declarations[0])
            && j.Identifier.check((statement.declarations[0]).id)
            && j.ConditionalExpression.check((statement.declarations[0]).init)
        ) {
            const declarator = statement.declarations[0]
            const identifier = declarator.id as Identifier
            if (getExistingDefaultParam(j, params, identifier.name)) return true

            const init = declarator.init as ConditionalExpression
            const initSource = j(init).toSource()

            const normalMatch = initSource.match(normalParameterRE)
            if (normalMatch) {
                const [_, length, index] = normalMatch
                if (length !== index) return true

                const exitingParam = getExistingParam(j, params, identifier.name)
                if (exitingParam) {
                    params.splice(params.indexOf(identifier), 1, identifier)
                    return false
                }

                const targetIndex = Number.parseInt(index, 10)
                params.splice(targetIndex, 0, identifier)
                return false
            }

            const defaultMatch = initSource.match(defaultParameterRE)
            if (defaultMatch) {
                const [_, length, index, index2] = defaultMatch
                // three numbers should be the the same
                if (length !== index || index !== index2 || index2 !== length) return true

                const defaultParam = init.alternate
                if (!defaultParam) return true

                const exitingParam = getExistingParam(j, params, identifier.name)
                if (exitingParam) {
                    params.splice(params.indexOf(identifier), 1, j.assignmentPattern(identifier, defaultParam))
                    return false
                }

                const targetIndex = Number.parseInt(index, 10)
                params.splice(targetIndex, 0, j.assignmentPattern(identifier, defaultParam))
                return false
            }
        }

        // TODO: rest parameter

        return true
    })

    path.node.body.body = [...filteredBodyInThreshold, ...bodyOutOfThreshold]
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

export default wrap(transformAST)
