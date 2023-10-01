import { findModuleFromSource } from '../../utils/import'
import { removeDeclarationIfUnused, removeDefaultImportIfUnused } from '../../utils/scope'
import wrap from '../../wrapAstTransformation'
import { isHelperFunctionCall } from './isHelperFunctionCall'
import type { ASTTransformation } from '../../wrapAstTransformation'
import type { Identifier } from '@babel/types'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { ImportDefaultSpecifier } from 'jscodeshift'

/**
 * Restores spread operator from `@babel/runtime/helpers/toConsumableArray` helper.
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-spread
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const moduleName = '@babel/runtime/helpers/toConsumableArray'
    const moduleEsmName = '@babel/runtime/helpers/esm/toConsumableArray'
    const moduleSource = findModuleFromSource(j, root, moduleName) || findModuleFromSource(j, root, moduleEsmName)

    if (moduleSource) {
        const isImport = j.ImportDeclaration.check(moduleSource)
        const moduleVariableName = isImport
            ? ((moduleSource.specifiers![0] as ImportDefaultSpecifier).local as Identifier).name
            : (moduleSource.id as Identifier).name

        // toConsumableArray(a)
        // toConsumableArray.default(a)
        // (0, toConsumableArray)(a)
        // (0, toConsumableArray.default)(a)
        root
            .find(j.CallExpression)
            .filter((path) => {
                return isHelperFunctionCall(j, path.node, moduleVariableName)
                && path.node.arguments.length === 1
                && j.Expression.check(path.node.arguments[0])
            })
            .forEach((path) => {
                path.replace(j.arrayExpression([j.spreadElement(path.node.arguments[0] as ExpressionKind)]))

                isImport
                    ? removeDefaultImportIfUnused(j, root, moduleVariableName)
                    : removeDeclarationIfUnused(j, path, moduleVariableName)
            })
    }
}

export default wrap(transformAST)
