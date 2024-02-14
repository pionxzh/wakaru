import { createAstGrepTransformationRule } from '@wakaru/shared/astGrepRule'

/**
 * Removes the `__esModule` flag from the module.
 *
 * @example
 * ```diff
 * - Object.defineProperty(exports, '__esModule', { value: true })
 * - exports.__esModule = !0
 * - module.exports.__esModule = true
 * ```
 */
export default createAstGrepTransformationRule({
    name: 'un-esmodule-flag',
    transform(root, s) {
        /**
         * Target: ES5+
         * Object.defineProperty(exports, '__esModule', { value: true })
         * Object.defineProperty(module.exports, '__esModule', { value: true })
         *
         * Target: ES3
         * exports.__esModule = true
         * module.exports.__esModule = true
         */
        root
            .findAll({
                rule: {
                    kind: 'expression_statement',
                    has: {
                        any: [
                            { pattern: `Object.defineProperty(exports, '__esModule', { value: $BOOL })` },
                            { pattern: `Object.defineProperty(exports, "__esModule", { value: $BOOL })` },

                            { pattern: `Object.defineProperty(module.exports, '__esModule', { value: $BOOL })` },
                            { pattern: `Object.defineProperty(module.exports, "__esModule", { value: $BOOL })` },

                            { pattern: `exports.__esModule = $BOOL` },
                            { pattern: `exports["__esModule"] = $BOOL` },
                            { pattern: `module.exports.__esModule = $BOOL` },
                            { pattern: `module.exports["__esModule"] = $BOOL` },
                        ],
                    },
                    // strip the trailing semicolon if any
                    regex: ';?$',
                },
                constraints: {
                    BOOL: {
                        regex: 'true|!0',
                    },
                },
            })
            .forEach((match) => {
                const range = match.range()
                s.remove(range.start.index, range.end.index)
            })

        return s
    },
})
