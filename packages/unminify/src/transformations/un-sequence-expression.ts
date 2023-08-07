import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

/**
 * Separate sequence expressions into multiple statements.
 *
 * @example
 * `a(), b(), c()` -> `a(); b(); c();`
 * `return a(), b()` -> `a(); return b()`
 *
 * @see https://babeljs.io/docs/en/babel-helper-to-multiple-sequence-expressions
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    // `return a(), b()` -> `a(); return b()`
    root
        .find(j.ReturnStatement, {
            argument: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { argument } } = path
            if (!j.SequenceExpression.check(argument)) return

            const { expressions } = argument
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            if (j.AssignmentExpression.check(last) && j.Identifier.check(last.left)) {
                replacement.push(j.expressionStatement(last))
                replacement.push(j.returnStatement(j.identifier(last.left.name)))
            }
            else {
                replacement.push(j.returnStatement(last))
            }

            if (j.IfStatement.check(path.parentPath.node)) {
                path.parentPath.replace(j.blockStatement(replacement))
            }
            else {
                j(path).replaceWith(replacement)
            }
        })

    // `if (a(), b(), c())` -> `a(); b(); if (c())`
    root
        .find(j.IfStatement, {
            test: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { test } } = path
            if (!j.SequenceExpression.check(test)) return

            const { expressions } = test
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.ifStatement(last, path.node.consequent, path.node.alternate))

            j(path).replaceWith(replacement)
        })

    // `while (a(), b(), c())` -> `a(); b(); while (c())`
    root
        .find(j.WhileStatement, {
            test: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { test } } = path
            if (!j.SequenceExpression.check(test)) return

            const { expressions } = test
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.whileStatement(last, path.node.body))

            j(path).replaceWith(replacement)
        })

    // `do { a(), b(), c() } while (d(), e(), f())` -> `a(); b(); do { c() } while (d(), e(), f())`
    root
        .find(j.DoWhileStatement, {
            test: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { test } } = path
            if (!j.SequenceExpression.check(test)) return

            const { expressions } = test
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.doWhileStatement(path.node.body, last))

            j(path).replaceWith(replacement)
        })

    // `switch (a(), b(), c())` -> `a(); b(); switch (c())`
    root
        .find(j.SwitchStatement, {
            discriminant: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { discriminant } } = path
            if (!j.SequenceExpression.check(discriminant)) return

            const { expressions } = discriminant
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.switchStatement(last, path.node.cases))

            j(path).replaceWith(replacement)
        })

    // `throw a(), b()` -> `a(); throw b()`
    root
        .find(j.ThrowStatement, {
            argument: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { argument } } = path
            if (!j.SequenceExpression.check(argument)) return

            const { expressions } = argument
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.throwStatement(last))

            j(path).replaceWith(replacement)
        })

    // `let x = (a(), b(), c())` -> `a(); b(); let x = c()`
    // `const x = (a(), b()), y = 1, z = (c(), d())` -> `a(); c(); const x = b(), y = 1, z = d()`
    root
        .find(j.VariableDeclaration, {
            declarations: [
                {
                    init: {
                        type: 'SequenceExpression',
                    },
                },
            ],
        })
        .forEach((path) => {
            if (j.ForStatement.check(path.parentPath.node)) return

            const { node: { declarations } } = path
            const replacement: any[] = []

            declarations.forEach((declaration) => {
                if (!j.VariableDeclarator.check(declaration)) {
                    replacement.push(j.variableDeclaration(path.node.kind, [declaration]))
                    return
                }
                const { init } = declaration
                if (!j.SequenceExpression.check(init)) {
                    replacement.push(j.variableDeclaration(path.node.kind, [declaration]))
                    return
                }

                const { expressions } = init
                const [last, ...rest] = [...expressions].reverse()
                replacement.push(...rest.reverse().map(e => j.expressionStatement(e)))
                replacement.push(j.variableDeclaration(path.node.kind, [j.variableDeclarator(declaration.id, last)]))
            })

            if (replacement.length > 0) {
                j(path).replaceWith(replacement)
            }
        })

    // `for (a(), b(); c(); d(), e()) {}` -> `a(); b(); for (; c(); ) { d(); e(); }`
    root
        .find(j.ForStatement, {
            init: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { init } } = path
            if (!j.SequenceExpression.check(init)) return

            const { expressions } = init
            const replacement: any[] = expressions.map(e => j.expressionStatement(e))
            replacement.push(j.forStatement(null, path.node.test, path.node.update, path.node.body))

            j(path).replaceWith(replacement)
        })

    // `for (let x = (a(), b(), c()), y = 1; x < 10; x++) {}` -> `a(); b(); for (let x = c(), y = 1; x < 10; x++) {}`
    root
        .find(j.ForStatement, {
            init: {
                type: 'VariableDeclaration',
                declarations: [
                    {
                        init: {
                            type: 'SequenceExpression',
                        },
                    },
                ],
            },
        })
        .forEach((path) => {
            const { node: { init } } = path
            if (!j.VariableDeclaration.check(init)) return
            const { declarations } = init
            const replacement: any[] = []
            const initDeclarators: any[] = []

            declarations.forEach((declaration) => {
                if (!j.VariableDeclarator.check(declaration)) {
                    replacement.push(j.variableDeclaration(init.kind, [declaration]))
                    return
                }
                const { init: declarationInit } = declaration
                if (!j.SequenceExpression.check(declarationInit)) {
                    initDeclarators.push(declaration)
                    return
                }

                const { expressions } = declarationInit
                const [last, ...rest] = [...expressions].reverse()
                replacement.push(...rest.reverse().map(e => j.expressionStatement(e)))
                initDeclarators.push(j.variableDeclarator(declaration.id, last))
            })

            if (replacement.length > 0) {
                replacement.push(j.forStatement(j.variableDeclaration(init.kind, initDeclarators), path.node.test, path.node.update, path.node.body))
                j(path).replaceWith(replacement)
            }
        })

    // `a(), b(), c()` -> `a(); b(); c();`
    root
        .find(j.ExpressionStatement, {
            expression: {
                type: 'SequenceExpression',
            },
        })
        .forEach((path) => {
            const { node: { expression } } = path
            if (!j.SequenceExpression.check(expression)) return

            const { expressions } = expression
            const replacement = expressions.map(e => j.expressionStatement(e))
            j(path).replaceWith(replacement)
        })
}

export default wrap(transformAST)
