import { findReferences } from '@unminify-kit/ast-utils'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import { findDeclaration, removeVariableDeclarator } from '../../../utils/scope'
import wrap from '../../../wrapAstTransformation'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '../../../wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTPath, AssignmentExpression, CallExpression, Identifier, VariableDeclarator } from 'jscodeshift'

/**
 * `@babel/runtime/helpers/createForOfIteratorHelper` helper.
 *
 * ```ts
 * function createForOfIteratorHelper(o, allowArrayLike?)
 * ```
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-for-of
 * @see https://github.com/babel/babel/blob/main/packages/babel-helpers/src/helpers.ts#L813-L872
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    const moduleName = '@babel/runtime/helpers/createForOfIteratorHelper'
    const moduleEsmName = '@babel/runtime/helpers/esm/createForOfIteratorHelper'

    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const helperLocals = findHelperLocals(context, params, moduleName, moduleEsmName)
    helperLocals.forEach((helperLocal) => {
        const references = findReferences(j, rootScope, helperLocal).length
        let processed = 0

        /**
         * var _iterator = babelHelpers.createForOfIteratorHelper(arr), _step;
         * try {
         *   for (_iterator.s(); !(_step = _iterator.n()).done;) {
         *     var result = _step.value;
         *   }
         * } catch (err) {
         *   _iterator.e(err);
         * } finally {
         *   _iterator.f();
         * }
         */
        root
            .find(j.VariableDeclarator)
            .filter(path => isHelperFunctionCall(j, path.node.init, helperLocal)
                 && path.node.init.arguments.length >= 1
                 && path.node.init.arguments.length <= 2
                 && j.Identifier.check(path.node.id),
            )
            .forEach((path) => {
                const scope = path.scope as Scope | null
                if (!scope) return

                // _iterator
                const _iterator = path.node.id as Identifier

                const array = (path.node.init as CallExpression).arguments[0] as Identifier

                // find `_step = _iterator.n()`
                const iteratorReferences = findReferences(j, scope, _iterator.name)
                const stepAssignments = iteratorReferences.filter((reference) => {
                    const assignment = reference.parent?.parent?.parent as ASTPath<AssignmentExpression>
                    if (!assignment) return false

                    return j.match(assignment, {
                        type: 'AssignmentExpression',
                        // @ts-expect-error no name provided
                        left: { type: 'Identifier' },
                        right: {
                            type: 'CallExpression',
                            callee: {
                                type: 'MemberExpression',
                                object: { type: 'Identifier', name: _iterator.name },
                                property: { type: 'Identifier', name: 'n' },
                            },
                            arguments: [],
                        },
                    })
                })
                if (stepAssignments.size() !== 1) return
                const _step: Identifier = stepAssignments.get().parent.parent.parent.node.left

                // find `var result = _step.value;`
                const stepReferences = findReferences(j, scope, _step.name)
                const resultDecls = stepReferences.filter((reference) => {
                    const declarator = reference.parent?.parent as ASTPath<VariableDeclarator>
                    if (!declarator) return false

                    return j.match(declarator, {
                        type: 'VariableDeclarator',
                        // @ts-expect-error no name provided
                        id: { type: 'Identifier' },
                        init: {
                            type: 'MemberExpression',
                            object: { type: 'Identifier', name: _step.name },
                            property: { type: 'Identifier', name: 'value' },
                        },
                    })
                })
                if (resultDecls.size() !== 1) return

                const tryStatements = resultDecls.closest(j.TryStatement)
                if (tryStatements.size() !== 1) return

                // Remove `var result = _step.value;`
                const _resultPath = resultDecls.get().parent.parent as ASTPath<VariableDeclarator>
                const _result: Identifier = _resultPath.node.id as Identifier
                removeVariableDeclarator(j, _resultPath.get('id'))

                // Remove _iterator's declaration
                const iteratorDecl = findDeclaration(scope, _iterator.name)
                if (iteratorDecl) removeVariableDeclarator(j, iteratorDecl)

                // Remove _step's declaration
                const stepDecl = findDeclaration(scope, _step.name)
                if (stepDecl) removeVariableDeclarator(j, stepDecl)

                // reconstruct for-of loop
                // 1. extract the `for...of` loop body
                const loopBody = resultDecls.closest(j.BlockStatement).get().node.body
                const forOfLoop = j.forOfStatement(
                    j.variableDeclaration('var', [
                        j.variableDeclarator(_result),
                    ]),
                    array,
                    j.blockStatement(loopBody),
                )
                tryStatements.get().replace(forOfLoop)

                processed += 1
            })

        if ((references - processed) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

export default wrap(transformAST)
