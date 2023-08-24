import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Converts `for(;;)` to `while(true)`.
 *
 * This is the reverse of the following transformation:
 * - SWC `jsc.minify.loop`
 * - Terser `compress.loops`
 * - ESBuild `minify: true`
 *
 * @example
 * for(;;) { ... }
 * ->
 * while(true) { ... }
 *
 * @see https://github.com/terser/terser/blob/27c0a3b47b429c605e2243df86044fc00815060f/test/compress/loops.js#L217
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.ForStatement, {
            init: null,
            test: null,
            update: null,
        })
        .forEach((p) => {
            p.replace(
                j.whileStatement(
                    j.booleanLiteral(true),
                    p.node.body,
                ),
            )
        })
}

export default wrap(transformAST)
