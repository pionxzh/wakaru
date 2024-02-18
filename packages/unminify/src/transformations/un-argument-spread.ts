import { isNull, isUndefined } from '@wakaru/ast-utils/matchers'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { Collection, Identifier, JSCodeshift, MemberExpression } from 'jscodeshift'

/**
 * Transform `fn.apply` calls to spread operator.
 *
 * Credits to `lebab` for the original implementation
 *
 * @see https://github.com/lebab/lebab/blob/master/src/transform/argSpread.js
 * @see https://babeljs.io/docs/babel-plugin-transform-parameters
 */
export default createJSCodeshiftTransformationRule({
    name: 'un-argument-spread',
    transform: (context) => {
        const { root, j } = context

        handleFunctionApplyCall(root, j)
        handleObjectApplyCall(root, j)
    },
})

/**
 * @example
 * ```ts
 * fn.apply(undefined, args);
 * fn.apply(null, args);
 * ```
 * ->
 * ```ts
 * fn(...args);
 * fn(...args);
 * ```
 */
function handleFunctionApplyCall(root: Collection, j: JSCodeshift) {
    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'Identifier',
                },
                property: {
                    type: 'Identifier',
                    name: 'apply',
                },
            },
            arguments: (args) => {
                if (args.length !== 2) return false

                return (isNull(j, args[0]) || isUndefined(j, args[0]))
                    && !j.SpreadElement.check(args[1])
            },
        })
        .forEach((path) => {
            const callee = path.value.callee as MemberExpression
            const fn = callee.object as Identifier
            const args = path.value.arguments[1] as ExpressionKind
            const spread = j.spreadElement(args)
            const callWithSpread = j.callExpression(fn, [spread])
            path.replace(callWithSpread)
        })
}

/**
 * @example
 * ```ts
 * obj.fn.apply(obj, args);
 * ```
 * ->
 * ```ts
 * obj.fn(...args);
 * ```
 */
function handleObjectApplyCall(root: Collection, j: JSCodeshift) {
    root
        .find(j.CallExpression, {
            callee: {
                type: 'MemberExpression',
                object: {
                    type: 'MemberExpression',
                },
                property: {
                    type: 'Identifier',
                    name: 'apply',
                },
            },
            arguments: (args) => {
                if (args.length !== 2) return false

                return !j.SpreadElement.check(args[0])
                    && !j.SpreadElement.check(args[1])
            },
        })
        .forEach((path) => {
            const callee = path.value.callee as MemberExpression
            const member = callee.object as MemberExpression
            const object = member.object
            const thisArg = path.value.arguments[0]
            if (object.type !== thisArg.type) return
            if (j.Identifier.check(object) && j.Identifier.check(thisArg) && object.name !== thisArg.name) return
            if (j(object).toSource() !== j(thisArg).toSource()) return

            const args = path.value.arguments[1] as ExpressionKind
            const spread = j.spreadElement(args)
            const callWithSpread = j.callExpression(member, [spread])
            path.replace(callWithSpread)
        })
}
