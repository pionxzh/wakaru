import { createAstGrepTransformationRule } from '@wakaru/shared/astGrepRule'

/**
 * Converts `1 / 0` to `Infinity`.
 *
 * @example
 * `1 / 0` -> `Infinity`
 *
 * @see https://babeljs.io/docs/babel-plugin-minify-infinity
 * @see Terser: `keep_infinity`
 * @see https://github.com/terser/terser/blob/931f8a5fd548795faae0da1fa9eafa3f2ad1647b/lib/compress/index.js#L2641
 */
export default createAstGrepTransformationRule({
    name: 'un-infinity',
    transform(root, s) {
        root
            .findAll({ rule: { pattern: '1/0' } })
            .forEach((match) => {
                const range = match.range()
                s.update(range.start.index, range.end.index, 'Infinity')
            })

        root
            .findAll({ rule: { pattern: '-1/0' } })
            .forEach((match) => {
                const range = match.range()
                s.update(range.start.index, range.end.index, '-Infinity')
            })

        return s
    },
})
