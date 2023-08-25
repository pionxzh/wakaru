import { transformToMultiStatementContext } from '../utils/transformToMultiStatementContext'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ForStatement } from 'jscodeshift'

/**
 * Separate variable declarators into multiple statements.
 *
 * @example
 * var a = 1, b = true, c = func(d)
 * ->
 * var a = 1
 * var b = true
 * var c = func(d)
 *
 * @example
 * // Separate variable declarators that are not used in for statements.
 * for (var i = 0, j = 0, k = 0; j < 10; k++) {}
 * ->
 * var i = 0
 * for (var j = 0, k = 0; j < 10; k++) {}
 *
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-merge-sibling-variables
 * @see https://github.com/terser/terser/blob/master/test/compress/join-vars.js
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.VariableDeclaration, {
            declarations: [
                {
                    type: 'VariableDeclarator',
                    id: { type: 'Identifier' },
                },
            ],
        })
        .forEach((p) => {
            if (p.parent?.node.type === 'ForStatement') {
                const { init, test, update } = p.parent.node as ForStatement
                if (init && j.VariableDeclaration.check(init) && init.kind === 'var') {
                    const initDeclarators = init.declarations
                    // filter out the declarations that are used in test or update
                    const usedDeclarators = initDeclarators.filter((d) => {
                        if (!j.VariableDeclarator.check(d)) return false

                        const { id } = d
                        if (!j.Identifier.check(id)) return false

                        const name = id.name
                        const isUsedInTest = test && j(test).find(j.Identifier, { name }).size() > 0
                        const isUsedInUpdate = update && j(update).find(j.Identifier, { name }).size() > 0
                        if (isUsedInTest || isUsedInUpdate) return true

                        return false
                    })

                    if (usedDeclarators.length === initDeclarators.length) return
                    init.declarations = usedDeclarators
                    if (init.declarations.length === 0) {
                        p.parent.node.init = null
                    }

                    const otherDeclarators = initDeclarators.filter(d => !usedDeclarators.includes(d))
                    const otherDeclarations = otherDeclarators.map(d => j.variableDeclaration(init.kind, [d]))
                    const replacements = [...otherDeclarations, p.parent.node]
                    transformToMultiStatementContext(j, p.parent, replacements)
                }

                return
            }

            const { kind, declarations } = p.node
            if (declarations.length <= 1) return

            const replacements = declarations.map(d => j.variableDeclaration(kind, [d]))
            transformToMultiStatementContext(j, p, replacements)
        })
}

export default wrap(transformAST)
