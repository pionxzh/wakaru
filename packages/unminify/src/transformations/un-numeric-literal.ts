import { wrapAstTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import type { ASTTransformation } from '@wakaru/ast-utils/wrapAstTransformation'

/**
 * Converts number literal to its decimal representation.
 * A comment will be added to the end of the line to indicate the original representation.
 *
 * Including:
 * - Decimal (Base 10)
 * - Float (Base 10)
 * - Binary (Base 2)
 * - Octal (Base 8)
 * - Hexadecimal (Base 16)
 * - Exponential notation
 *
 * @example
 * 0b101010 -> 42
 * 0o777 -> 511
 * 0x123 -> 291
 * 1e3 -> 1000
 *
 * @see https://babeljs.io/docs/babel-plugin-minify-numeric-literals
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.NumericLiteral)
        .forEach((path) => {
            const node = path.node
            const { value, extra: { raw } = {} } = node

            if (raw && raw !== value.toString()) {
                const originalComment = path.node.comments || []
                const operator = j.UnaryExpression.check(path.parent.node) && path.parent.node.operator === '-'
                    ? '-'
                    : ''
                const comment = j.commentBlock(` ${operator}${raw} `, false, true)
                const decimalRepresentation = j.numericLiteral(value)
                decimalRepresentation.comments = [...originalComment, comment]
                path.replace(decimalRepresentation)
            }
        })
}

export default wrapAstTransformation(transformAST)
