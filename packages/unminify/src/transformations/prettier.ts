import { createStringTransformationRule } from '@wakaru/shared/rule'
import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'

/**
 * @url https://prettier.io
 */
export default createStringTransformationRule({
    name: 'prettier',
    transform: (code) => {
        return prettier.format(code, {
            parser: 'babel',
            plugins: [babelParser],
        })
    },
})
