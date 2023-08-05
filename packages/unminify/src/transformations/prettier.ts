import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * @url https://prettier.io
 */
export const transformAST: ASTTransformation = (context) => {
    const code = context.root.toSource()
    return prettier.format(code, {
        parser: 'babel',
        plugins: [babelParser],
    })
}

export default wrap(transformAST)
