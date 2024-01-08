import { createAstGrepTransformationRule } from '@wakaru/shared/rule'

/**
 * Converts `void 0` to `undefined`.
 *
 * @example
 * void 0 -> undefined
 * void 99 -> undefined
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-undefined-to-void
 * @see Terser: `unsafe_undefined`
 */
export default createAstGrepTransformationRule({
    name: 'un-undefined',
    transform(root, s) {
        root
            .findAll({
                rule: {
                    any: [
                        { pattern: 'void ($NUMBER)' },
                        { pattern: 'void $NUMBER' },
                    ],
                },
                constraints: {
                    NUMBER: { kind: 'number' },
                },
            })
            .forEach((match) => {
                const range = match.range()
                s.update(range.start.index, range.end.index, 'undefined')
            })

        return s
    },
})
