import { isNumber } from '@unminify-kit/ast-utils'
import { findModuleFromSource } from '../../utils/import'
import { removeDeclarationIfUnused, removeDefaultImportIfUnused } from '../../utils/scope'
import wrap from '../../wrapAstTransformation'
import { isHelperFunctionCall } from './isHelperFunctionCall'
import type { ASTTransformation } from '../../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { CallExpression, Identifier, ImportDefaultSpecifier, Literal, VariableDeclarator } from 'jscodeshift'

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
