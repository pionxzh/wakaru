import { findReferences } from '@unminify-kit/ast-utils'
import { fromPaths } from 'jscodeshift/src/Collection'
import { generateName } from '../../../utils/identifier'
import { findHelperLocals, removeHelperImport } from '../../../utils/import'
import { isHelperFunctionCall } from '../../../utils/isHelperFunctionCall'
import { findDeclaration, removeVariableDeclarator } from '../../../utils/scope'
import wrap from '../../../wrapAstTransformation'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '../../../wrapAstTransformation'
import type { StatementKind } from 'ast-types/lib/gen/kinds'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTPath, AssignmentExpression, CallExpression, ForStatement, Identifier, JSCodeshift, MemberExpression, TryStatement, VariableDeclarator } from 'jscodeshift'

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
    const looseModuleName = '@babel/runtime/helpers/createForOfIteratorHelperLoose'
    const looseModuleEsmName = '@babel/runtime/helpers/esm/createForOfIteratorHelperLoose'

    const { root, j } = context
    const rootScope = root.find(j.Program).get().scope as Scope | null
    if (!rootScope) return

    const helperLocals = [
        ...findHelperLocals(context, params, moduleName, moduleEsmName),
        ...findHelperLocals(context, params, looseModuleName, looseModuleEsmName),
    ]
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

                const forOfResult = findForOf(j, path, scope) || findForOfLoose(j, path, scope)
                if (!forOfResult) return

                const { object, _iterator, _step, _resultPath, body, containerPath } = forOfResult

                // Remove `var result = _step.value;`
                const _result = _resultPath.node
                if (j.Identifier.check(_resultPath.node)) {
                    removeVariableDeclarator(j, _resultPath as ASTPath<Identifier>)
                }
                else {
                    fromPaths([_resultPath]).closest(j.AssignmentExpression).remove()
                }

                // Remove _iterator's declaration
                const iteratorDecl = findDeclaration(scope, _iterator.name)
                if (iteratorDecl) removeVariableDeclarator(j, iteratorDecl)

                // Remove _step's declaration
                const stepDecl = findDeclaration(scope, _step.name)
                if (stepDecl) removeVariableDeclarator(j, stepDecl)

                // reconstruct for-of loop
                // 1. extract the `for...of` loop body
                let left
                if (j.Identifier.check(_result)) {
                    left = j.variableDeclaration('var', [j.variableDeclarator(_result)])
                }
                else {
                    const tempVariableName = generateName('_value', scope)
                    left = j.identifier(tempVariableName)
                    const assignment = j.assignmentExpression('=', left, _result)
                    body.unshift(j.expressionStatement(assignment))
                }
                const forOfLoop = j.forOfStatement(
                    left,
                    object,
                    j.blockStatement(body),
                )
                containerPath.replace(forOfLoop)

                processed += 1
            })

        if ((references - processed) === 1) {
            removeHelperImport(j, rootScope, helperLocal)
        }
    })
}

interface ForOfResult {
    object: Identifier
    _iterator: Identifier
    _step: Identifier
    _resultPath: ASTPath<Identifier | MemberExpression>
    body: StatementKind[]
    containerPath: ASTPath<ForStatement | TryStatement>
}

function findForOf(j: JSCodeshift, path: ASTPath<VariableDeclarator>, scope: Scope): ForOfResult | null {
    // _iterator
    const _iterator = path.node.id as Identifier

    const object = (path.node.init as CallExpression).arguments[0] as Identifier

    // find `_step = _iterator.n()`
    const iteratorReferences = findReferences(j, scope, _iterator.name)
    const stepAssignments = iteratorReferences.filter((reference) => {
        const assignment = reference.parent?.parent?.parent as ASTPath<AssignmentExpression>
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
    if (stepAssignments.size() !== 1) return null
    const _step: Identifier = stepAssignments.get().parent.parent.parent.node.left

    // find `var result = _step.value;`
    const stepReferences = findReferences(j, scope, _step.name)
    const resultDecls = stepReferences.filter((reference) => {
        const declarator = reference.parent?.parent as ASTPath<VariableDeclarator>
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
        // @ts-expect-error no left provided
        || j.match(declarator, {
            type: 'AssignmentExpression',
            right: {
                type: 'MemberExpression',
                object: { type: 'Identifier', name: _step.name },
                property: { type: 'Identifier', name: 'value' },
            },
        })
    })
    if (resultDecls.size() !== 1) return null
    const resultDecl = resultDecls.get().parent.parent
    const _resultPath = j.Identifier.check(resultDecl.node.id)
        ? resultDecl.get('id')
        : resultDecl.get('left')

    const body = resultDecls.closest(j.BlockStatement).get().node.body

    const containerPaths = resultDecls.closest(j.TryStatement)
    if (containerPaths.size() !== 1) return null
    const containerPath = containerPaths.get()

    return {
        object,
        _iterator,
        _step,
        _resultPath,
        body,
        containerPath,
    }
}

function findForOfLoose(j: JSCodeshift, path: ASTPath<VariableDeclarator>, scope: Scope): ForOfResult | null {
    // _iterator
    const _iterator = path.node.id as Identifier

    const object = (path.node.init as CallExpression).arguments[0] as Identifier

    // find `_step = _iterator()`
    const iteratorReferences = findReferences(j, scope, _iterator.name)
    const stepAssignments = iteratorReferences.filter((reference) => {
        // _step = _iterator()
        const assignment = reference.parent?.parent?.node
        return j.match(assignment, {
            type: 'AssignmentExpression',
            // @ts-expect-error no name provided
            left: { type: 'Identifier' },
            right: {
                type: 'CallExpression',
                callee: { type: 'Identifier', name: _iterator.name },
                arguments: [],
            },
        })
    })
    if (stepAssignments.size() !== 1) return null
    const _step: Identifier = stepAssignments.get().parent.parent.node.left

    // find `var result = _step.value;`
    const stepReferences = findReferences(j, scope, _step.name)
    const resultDecls = stepReferences.filter((reference) => {
        const declarator = reference.parent?.parent as ASTPath<VariableDeclarator>
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
        // @ts-expect-error no left provided
        || j.match(declarator, {
            type: 'AssignmentExpression',
            right: {
                type: 'MemberExpression',
                object: { type: 'Identifier', name: _step.name },
                property: { type: 'Identifier', name: 'value' },
            },
        })
    })
    if (resultDecls.size() !== 1) return null
    const resultDecl = resultDecls.get().parent.parent
    const _resultPath = j.Identifier.check(resultDecl.node.id)
        ? resultDecl.get('id')
        : resultDecl.get('left')

    const body = resultDecls.closest(j.BlockStatement).get().node.body

    const containerPaths = resultDecls.closest(j.ForStatement)
    if (containerPaths.size() !== 1) return null
    const containerPath = containerPaths.get()

    return {
        object,
        _iterator,
        _step,
        _resultPath,
        body,
        containerPath,
    }
}

export default wrap(transformAST)
