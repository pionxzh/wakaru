import { isNumber } from '@unminify-kit/ast-utils'
import { findModuleFromSource } from '../../utils/import'
import { removeDeclarationIfUnused, removeDefaultImportIfUnused } from '../../utils/scope'
import wrap from '../../wrapAstTransformation'
import type { ASTTransformation } from '../../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { CallExpression, Identifier, ImportDefaultSpecifier, JSCodeshift, Literal, VariableDeclarator } from 'jscodeshift'

/**
 * Checks if the expression is a call to the helper function.
 *
 * @example
 * // matches
 * helperName(...)
 * helperName.default(...)
 * (0, helperName)(...)
 * (0, helperName.default)(...)
 */
function isHelperFunctionCall(
    j: JSCodeshift,
    expression: ExpressionKind | null | undefined,
    helperName: string,
): expression is CallExpression {
    if (!j.CallExpression.check(expression)) return false

    let callee = expression.callee
    if (j.SequenceExpression.check(callee)) {
        if (
            callee.expressions.length === 2
            && j.Literal.check(callee.expressions[0])
            && callee.expressions[0].value === 0
        ) {
            callee = callee.expressions[1]
        }
    }

    if (j.Identifier.check(callee)) {
        return callee.name === helperName
    }
    if (j.MemberExpression.check(callee)) {
        return (
            j.Identifier.check(callee.object)
            && callee.object.name === helperName
            && j.Identifier.check(callee.property)
            && callee.property.name === 'default'
        )
    }

    return false
}

/**
 * Restores array destructuring from `@babel/runtime/helpers/slicedToArray` helper.
 *
 * TODO: improve `for...of` loops output.
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-destructuring
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const moduleName = '@babel/runtime/helpers/slicedToArray'
    const moduleEsmName = '@babel/runtime/helpers/esm/slicedToArray'
    const moduleSource = findModuleFromSource(j, root, moduleName) || findModuleFromSource(j, root, moduleEsmName)

    if (moduleSource) {
        const isImport = j.ImportDeclaration.check(moduleSource)
        const moduleVariableName = isImport
            ? ((moduleSource.specifiers![0] as ImportDefaultSpecifier).local as Identifier).name
            : (moduleSource.id as Identifier).name

        // var _ref = slicedToArray(a, 2)
        // var _ref = slicedToArray.default(a, 2)
        // var _ref = (0, slicedToArray)(a, 2)
        // var _ref = (0, slicedToArray.default)(a, 2)
        root
            .find(j.VariableDeclaration, {
                declarations: (declarations) => {
                    return declarations.length === 1
                    && j.VariableDeclarator.check(declarations[0])
                    && j.Identifier.check(declarations[0].id)
                    && isHelperFunctionCall(j, declarations[0].init, moduleVariableName)

                    && declarations[0].init.arguments.length === 2
                    && j.Literal.check(declarations[0].init.arguments[1])
                    && isNumber(declarations[0].init.arguments[1].value)
                },
            })
            .forEach((path) => {
                const decl = path.node.declarations[0] as VariableDeclarator
                const tempVariable = decl.id as Identifier
                const wrappedExpression = (decl.init as CallExpression).arguments[0] as ExpressionKind
                const length = ((decl.init as CallExpression).arguments[1] as Literal).value as number

                if (length === 0) {
                    // var [] = wrappedExpression
                    path.replace(j.variableDeclaration(path.node.kind, [
                        j.variableDeclarator(
                            j.arrayPattern([]),
                            wrappedExpression,
                        ),
                    ]))
                }
                else {
                    path.replace(j.variableDeclaration(path.node.kind, [
                        j.variableDeclarator(
                            tempVariable,
                            wrappedExpression,
                        ),
                    ]))
                }

                isImport
                    ? removeDefaultImportIfUnused(j, root, moduleVariableName)
                    : removeDeclarationIfUnused(j, path, moduleVariableName)
            })
    }
}

export default wrap(transformAST)
