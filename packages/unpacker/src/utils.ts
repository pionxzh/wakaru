import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'

export function prettierFormat(code: string) {
    return prettier.format(code, {
        parser: 'babel',
        plugins: [babelParser],
    })
}
