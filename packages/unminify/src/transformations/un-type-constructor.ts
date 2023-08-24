import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Restore type constructors from minified code.
 *
 * @example
 * +x; // Number(x)
 * x + ""; // String(x)
 * [,,,]; // Array(3)
 *
 * @example
 * // We don't transform the following cases because the original code is more readable.
 *
 * !!x; // Boolean(x)
 * [3, 1]; // Array.of(3, 1)
 * {foo: 'bar'}; // Object({foo: 'bar'})
 *
 * @see https://babeljs.io/docs/babel-plugin-minify-type-constructors
 */
export const transformAST: ASTTransformation = (context) => {
    // const { root, j } = context

    // TODO: implement
}

export default wrap(transformAST)
