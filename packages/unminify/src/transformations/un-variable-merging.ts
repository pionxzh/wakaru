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
 * @see https://babeljs.io/docs/en/babel-plugin-transform-merge-sibling-variables
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
                    const initDeclarations = init.declarations
                    // filter out the declarations that are used in test or update
                    const usedDeclarations = initDeclarations.filter((d) => {
                        if (!j.VariableDeclarator.check(d)) return false

                        const { id } = d
                        if (!j.Identifier.check(id)) return false

                        const name = id.name
                        const isUsedInTest = test && j(test).find(j.Identifier, { name }).size() > 0
                        const isUsedInUpdate = update && j(update).find(j.Identifier, { name }).size() > 0
                        if (isUsedInTest || isUsedInUpdate) return true

                        return false
                    })

                    if (usedDeclarations.length === initDeclarations.length) return
                    init.declarations = usedDeclarations

                    const otherDeclarations = initDeclarations.filter(d => !usedDeclarations.includes(d))
                    j(p.parent).insertBefore(j.variableDeclaration(init.kind, otherDeclarations))
                }

                return
            }

            const { kind, declarations } = p.node
            if (declarations.length <= 1) return

            j(p).replaceWith(declarations.map(d => j.variableDeclaration(kind, [d])))
        })
}

export default wrap(transformAST)
