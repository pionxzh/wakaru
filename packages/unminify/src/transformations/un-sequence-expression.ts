import { mergeComments } from '../utils/comments'
import { replaceWithMultipleStatements } from '../utils/insert'
import { smartParenthesized } from '../utils/parenthesized'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { AssignmentExpression, Identifier, MemberExpression, SequenceExpression, VariableDeclaration } from 'jscodeshift'

/**
 * Separate sequence expressions into multiple statements.
 *
 * @example
 * `a(), b(), c()` -> `a(); b(); c();`
 * `return a(), b()` -> `a(); return b()`
 *
 * @see https://babeljs.io/docs/babel-helper-to-multiple-sequence-expressions
 * @see https://github.com/terser/terser/blob/master/test/compress/sequences.js
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    // () => (a(), b(), c()) -> () => { a(); b(); return c() }
    root
        .find(j.ArrowFunctionExpression, { body: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const body = path.node.body as SequenceExpression

            const { expressions } = body
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            if (j.AssignmentExpression.check(last) && (j.Identifier.check(last.left) || j.MemberExpression.check(last.left))) {
                replacement.push(j.expressionStatement(last))
                replacement.push(j.returnStatement(last.left))
            }
            else {
                replacement.push(j.returnStatement(last))
            }

            mergeComments(replacement, path.node.comments)
            path.node.body = j.blockStatement(replacement)
        })

    // `return a(), b()` -> `a(); return b()`
    root
        .find(j.ReturnStatement, { argument: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const argument = path.node.argument as SequenceExpression

            const { expressions } = argument
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            if (j.AssignmentExpression.check(last) && (j.Identifier.check(last.left) || j.MemberExpression.check(last.left))) {
                replacement.push(j.expressionStatement(last))
                replacement.push(j.returnStatement(last.left))
            }
            else {
                replacement.push(j.returnStatement(last))
            }

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })

    // `if (a(), b(), c())` -> `a(); b(); if (c())`
    root
        .find(j.IfStatement, { test: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const test = path.node.test as SequenceExpression

            const { expressions } = test
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.ifStatement(last, path.node.consequent, path.node.alternate))

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })

    // `while (a(), b(), c())` -> `a(); b(); while (c())`
    root
        .find(j.WhileStatement, { test: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const test = path.node.test as SequenceExpression

            const { expressions } = test
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.whileStatement(last, path.node.body))

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })

    // `do { a(), b(), c() } while (d(), e(), f())` -> `a(); b(); do { c() } while (d(), e(), f())`
    root
        .find(j.DoWhileStatement, { test: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const test = path.node.test as SequenceExpression

            const { expressions } = test
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.doWhileStatement(path.node.body, last))

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })

    // `switch (a(), b(), c())` -> `a(); b(); switch (c())`
    root
        .find(j.SwitchStatement, { discriminant: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const discriminant = path.node.discriminant as SequenceExpression

            const { expressions } = discriminant
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.switchStatement(last, path.node.cases))

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })

    // `throw a(), b()` -> `a(); throw b()`
    root
        .find(j.ThrowStatement, { argument: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const argument = path.node.argument as SequenceExpression

            const { expressions } = argument
            const [last, ...rest] = [...expressions].reverse()
            const replacement: any[] = rest.reverse().map(e => j.expressionStatement(e))
            replacement.push(j.throwStatement(last))

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })

    // `let x = (a(), b(), c())` -> `a(); b(); let x = c()`
    // `const x = (a(), b()), y = 1, z = (c(), d())` -> `a(); c(); const x = b(), y = 1, z = d()`
    root
        .find(j.VariableDeclaration, {
            declarations: [
                { init: { type: 'SequenceExpression' } },
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
                mergeComments(replacement, path.node.comments)
                j(path).replaceWith(replacement)
            }
        })

    // `for (a(), b(); c(); d(), e()) {}` -> `a(); b(); for (; c(); ) { d(); e(); }`
    root
        .find(j.ForStatement, { init: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const init = path.node.init as SequenceExpression

            const { expressions } = init
            const replacement: any[] = expressions.map(e => j.expressionStatement(e))
            replacement.push(j.forStatement(null, path.node.test, path.node.update, path.node.body))

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })

    // `for (let x = (a(), b(), c()), y = 1; x < 10; x++) {}` -> `a(); b(); for (let x = c(), y = 1; x < 10; x++) {}`
    root
        .find(j.ForStatement, {
            init: {
                type: 'VariableDeclaration',
                declarations: [
                    { init: { type: 'SequenceExpression' } },
                ],
            },
        })
        .forEach((path) => {
            const init = path.node.init as VariableDeclaration
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
                mergeComments(replacement, path.node.comments)
                replaceWithMultipleStatements(j, path, replacement)
            }
        })

    // `a(), b(), c()` -> `a(); b(); c();`
    root
        .find(j.ExpressionStatement, { expression: { type: 'SequenceExpression' } })
        .forEach((path) => {
            const expression = path.node.expression as SequenceExpression

            const { expressions } = expression
            const replacement = expressions.map(e => j.expressionStatement(e))

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })

    // (a = b())['c'] = d -> a = b(); a['c'] = d
    root
        .find(j.ExpressionStatement, {
            expression: {
                type: 'AssignmentExpression',
                left: {
                    type: 'MemberExpression',
                    object: {
                        type: 'AssignmentExpression',
                        left: {
                            type: 'Identifier',
                        },
                    },
                },
            },
        })
        .forEach((path) => {
            const { left, right } = path.node.expression as AssignmentExpression
            const { object, property, computed } = left as MemberExpression
            const ident = (object as AssignmentExpression).left as Identifier

            const extracted = j.expressionStatement(smartParenthesized(j, object))
            const assignment = j.expressionStatement(
                j.assignmentExpression(
                    '=',
                    j.memberExpression(ident, property, computed),
                    right,
                ),
            )
            const replacement = [extracted, assignment]

            mergeComments(replacement, path.node.comments)
            replaceWithMultipleStatements(j, path, replacement)
        })
}

export default wrap(transformAST)
