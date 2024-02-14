import { createAstGrepTransformationRule } from '@wakaru/shared/astGrepRule'

/**
 * Converts minified `boolean` to simple `true`/`false`.
 *
 * @example
 * !0 -> true
 * !1 -> false
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-minify-booleans
 * @see Terser: `booleans_as_integers`
 */
export default createAstGrepTransformationRule({
    name: 'un-boolean',
    transform(root, s) {
        root
            .findAll({ rule: { pattern: '!0' } })
            .forEach((match) => {
                const range = match.range()
                s.update(range.start.index, range.end.index, 'true')
            })

        root
            .findAll({ rule: { pattern: '!1' } })
            .forEach((match) => {
                const range = match.range()
                s.update(range.start.index, range.end.index, 'false')
            })

        return s
    },
})
