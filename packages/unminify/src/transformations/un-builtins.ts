import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Inline extracted built-in Object static methods.
 *
 * @example
 * var _Mathfloor = Math.floor;
 * _Mathfloor(a) + _Mathfloor(b);
 * ->
 * Math.floor(a) + Math.floor(b);
 *
 * @see https://babeljs.io/docs/babel-plugin-minify-builtins
 */
export const transformAST: ASTTransformation = (_context) => {
    // const { root, j } = context

    // https://github.com/babel/minify/tree/master/packages/babel-plugin-minify-builtins
    // TODO: implement
}

export default wrap(transformAST)
