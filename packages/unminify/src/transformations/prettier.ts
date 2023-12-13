import { wrapStringTransformation } from '@wakaru/ast-utils/wrapStringTransformation'
import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'

/**
 * @url https://prettier.io
 */
export default wrapStringTransformation((code) => {
    return prettier.format(code, {
        parser: 'babel',
        plugins: [babelParser],
    })
})
