import { mergeComments } from '@wakaru/ast-utils/comments'
import { replaceWithMultipleStatements } from '@wakaru/ast-utils/insert'
import { findReferences } from '@wakaru/ast-utils/reference'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import type { ASTTransformation } from '@wakaru/shared/rule'
import type { ASTPath, ForStatement } from 'jscodeshift'

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
        .find(j.VariableDeclaration)
        .forEach((p) => {
            if (j.ForStatement.check(p.parent.node)) {
                const forPath = p.parent as ASTPath<ForStatement>
                const { init } = forPath.node
                if (init && j.VariableDeclaration.check(init) && init.kind === 'var') {
                    const initDeclarators = init.declarations
                    // filter out the declarations that are used in test or update
                    const usedDeclarators = initDeclarators.filter((declarator) => {
                        if (!j.VariableDeclarator.check(declarator)) return false

                        const { id } = declarator
                        if (!j.Identifier.check(id)) return false

                        // check if the name is declared outside of the for statement
                        const forParent = forPath?.parent
                        const isDeclaredInForParent = j(forParent)
                            .find(j.VariableDeclarator, {
                                id: {
                                    type: 'Identifier',
                                    name: id.name,
                                },
                            })
                            .filter(pp => pp.parent.parent === forParent)
                            .size() > 0
                        if (isDeclaredInForParent) return true

                        const isUsed = findReferences(j, p.parent, id.name).size() > 1
                        if (isUsed) return true

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
                    // seems no comments can be being to extracted statements
                    // mergeComments(replacements, p.node.comments)

                    replaceWithMultipleStatements(j, p.parent, replacements)
                }

                return
            }

            if (j.ExportNamedDeclaration.check(p.parent.node)) {
                const { kind, declarations } = p.node
                if (declarations.length <= 1) return

                const replacements = declarations.map(d => j.exportNamedDeclaration(j.variableDeclaration(kind, [d])))
                mergeComments(replacements, p.node.comments)

                replaceWithMultipleStatements(j, p.parent, replacements)
            }

            const { kind, declarations } = p.node
            if (declarations.length <= 1) return

            const replacements = declarations.map(d => j.variableDeclaration(kind, [d]))
            mergeComments(replacements, p.node.comments)

            replaceWithMultipleStatements(j, p, replacements)
        })
}

export default createJSCodeshiftTransformationRule({
    name: 'un-variable-merging',
    transform: transformAST,
})
