import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Converts for loop without init and update to while loop.
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
 * @example
 * for(; ? ;) { ... }
 * ->
 * while(? ) { ... }
 *
 * @see Terser: `loops`
 * @see https://github.com/terser/terser/blob/27c0a3b47b429c605e2243df86044fc00815060f/test/compress/loops.js#L217
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    // for(; ? ;) { ... }
    root
        .find(j.ForStatement, {
            init: null,
            update: null,
        })
        .forEach((p) => {
            const test = p.node.test ?? j.booleanLiteral(true)
            p.replace(
                j.whileStatement(
                    test,
                    p.node.body,
                ),
            )
        })
}

export default wrap(transformAST)
