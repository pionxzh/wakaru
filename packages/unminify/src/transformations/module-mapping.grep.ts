import { createAstGrepTransformationRule } from '@wakaru/shared/rule'

/**
 * // params: { 29: 'index.js' }
 * const a = require(29)
 * ->
 * const a = require('index.js')
 */
export default createAstGrepTransformationRule({
    name: 'module-mapping',
    transform(root, s, params) {
        const { moduleMapping = {} } = params

        root
            .findAll({
                rule: {
                    pattern: 'require($SOURCE)',
                },
            })
            .forEach((match) => {
                const node = match.getMatch('SOURCE')
                if (!node) return

                // key can be a number or a string
                // we want to remove the quotes from the string
                const key = node.text().replace(/^['"]|['"]$/g, '')
                const replacement = moduleMapping[key]
                if (!replacement) return

                const range = node.range()
                s.update(range.start.index, range.end.index, `"${replacement}"`)
            })

        return s
    },
})
