import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { Literal } from 'jscodeshift'

/**
 * // params: { 29: 'index.js' }
 * const a = require(29)
 * ->
 * const a = require('index.js')
 */
interface Params {
    moduleMapping: Record<number | string, string>
}
export const transformAST: ASTTransformation<Params> = (context, params = { moduleMapping: {} }) => {
    const { root, j } = context
    const { moduleMapping = {} } = params

    root
        .find(j.CallExpression, {
            callee: {
                type: 'Identifier',
                name: 'require',
            },
            arguments: args => args.length === 1 && j.Literal.check(args[0]),
        })
        .forEach((p) => {
            const { value } = p.node.arguments[0] as Literal
            if (typeof value !== 'number' && typeof value !== 'string') return

            const replacement = moduleMapping[value]
            if (typeof replacement !== 'undefined') {
                p.node.arguments[0] = j.literal(`default-${replacement}.js`)
            }
        })
}

export default wrap(transformAST)
