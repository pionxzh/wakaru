import prettier from 'prettier/standalone'
import babelParser from 'prettier/parser-babel'
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
