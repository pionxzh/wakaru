import { createAstGrepTransformationRule } from '@wakaru/shared/rule'

/**
 * Remove the 'use strict' directives
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-minify-booleans
 */
export default createAstGrepTransformationRule({
    name: 'un-use-strict',
    transform(root, s) {
        root
            .findAll({
                rule: {
                    regex: 'use strict',
                    kind: 'string',
                    inside: {
                        kind: 'expression_statement',
                    },
                },
            })
            .forEach((match) => {
                const range = match.range()
                s.remove(range.start.index - range.start.column, range.end.index)
            })

        return s
    },
})
