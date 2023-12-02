import { wrapAstTransformation } from '@wakaru/ast-utils/wrapAstTransformation'
import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'
import type { ASTTransformation } from '@wakaru/ast-utils/wrapAstTransformation'

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

export default wrapAstTransformation(transformAST)
