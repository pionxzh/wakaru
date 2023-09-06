import { removeDeclarationIfUnused, removeDefaultImportIfUnused } from '../../utils/removeDeclarationIfUnused'
import wrap from '../../wrapAstTransformation'
import type { ASTTransformation } from '../../wrapAstTransformation'
import type { Identifier } from '@babel/types'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { CallExpression, Collection, ImportDeclaration, ImportDefaultSpecifier, JSCodeshift, VariableDeclarator } from 'jscodeshift'

/**
 * Restores spread operator from `@babel/runtime/helpers/toConsumableArray` helper.
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-spread
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    const moduleName = '@babel/runtime/helpers/toConsumableArray'
    const moduleEsmName = '@babel/runtime/helpers/esm/toConsumableArray'
    const moduleSource = findModuleSource(j, root, moduleName) || findModuleSource(j, root, moduleEsmName)

    if (moduleSource) {
        const isESM = j.ImportDeclaration.check(moduleSource)
        const moduleVariableName = isESM
            ? ((moduleSource.specifiers![0] as ImportDefaultSpecifier).local as Identifier).name
            : (moduleSource.id as Identifier).name

        // toConsumableArray(a)
        // toConsumableArray.default(a)
        root
            .find(j.CallExpression, {
                callee: (callee: CallExpression['callee']) => {
                    return (
                        j.Identifier.check(callee)
                     && callee.name === moduleVariableName
                    )
                || (
                    j.MemberExpression.check(callee)
                    && j.Identifier.check(callee.object)
                    && callee.object.name === moduleVariableName
                    && j.Identifier.check(callee.property)
                    && callee.property.name === 'default'
                )
                },
                arguments: (args: CallExpression['arguments']) => args.length === 1 && j.Expression.check(args[0]),
            })
            .forEach((path) => {
                path.replace(j.arrayExpression([j.spreadElement(path.node.arguments[0] as ExpressionKind)]))
                isESM
                    ? removeDefaultImportIfUnused(j, path, moduleVariableName)
                    : removeDeclarationIfUnused(j, path, moduleVariableName)
            })

        // (0, toConsumableArray)(a)
        // (0, toConsumableArray.default)(a)
        root
            .find(j.CallExpression, {
                callee: {
                    type: 'SequenceExpression',
                    expressions: [
                        { type: 'Literal', value: 0 },
                        (expression: any) => {
                            return (
                                j.Identifier.check(expression)
                             && expression.name === moduleVariableName
                            )
                        || (
                            j.MemberExpression.check(expression)
                            && j.Identifier.check(expression.object)
                            && expression.object.name === moduleVariableName
                            && j.Identifier.check(expression.property)
                            && expression.property.name === 'default'
                        )
                        },
                    ],
                },
                arguments: (args: CallExpression['arguments']) => args.length === 1 && j.Expression.check(args[0]),
            })
            .forEach((path) => {
                path.replace(j.arrayExpression([j.spreadElement(path.node.arguments[0] as ExpressionKind)]))
                isESM
                    ? removeDefaultImportIfUnused(j, path, moduleVariableName)
                    : removeDeclarationIfUnused(j, path, moduleVariableName)
            })
    }
}

/**
 * Find the module source of given module name.
 */
function findModuleSource(j: JSCodeshift, root: Collection, moduleName: string): ImportDeclaration | VariableDeclarator | null {
    // import mod from 'moduleName'
    const importDeclarations = root.find(j.ImportDeclaration, {
        specifiers: [{ type: 'ImportDefaultSpecifier', local: { type: 'Identifier' } }],
        source: { type: 'Literal', value: moduleName },
    })
    if (importDeclarations.size() > 0) {
        return importDeclarations.get().node
    }

    // const mod = require('moduleName')
    const variableDeclarators = root.find(j.VariableDeclarator, {
        init: {
            type: 'CallExpression',
            callee: { type: 'Identifier', name: 'require' },
            arguments: [{ type: 'Literal', value: moduleName } as const],
        },
    })
    if (variableDeclarators.size() > 0) {
        return variableDeclarators.get().node
    }

    return null
}

export default wrap(transformAST)
