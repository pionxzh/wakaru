import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ArrayExpression, CallExpression, ExpressionStatement, FunctionExpression, Identifier, Literal, ReturnStatement, SwitchStatement, YieldExpression } from 'jscodeshift'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'

// cSpell:words trys endfinally

/**
  * Restore `async` and `await` keywords
  *
  * Restore tslib helper __generator and __awaiter
  *
  * @example
  * function asyncAwait() {
  *   return __awaiter(this, void 0, void 0, function () {
  *     var result, json;
  *     return __generator(this, function (_a) {
  *         switch (_a.label) {
  *             case 0:
  *                 console.log('Before sleep');
  *                 return [4 /*yield* /, sleep(1000)];
  *                 case 1:
  *                     _a.sent();
  *                     return [4 /*yield* /, fetch('')];
  *                 case 2:
  *                     result = _a.sent();
  *                     return [4 /*yield* /, result.json()];
  *                 case 3:
  *                    json = _a.sent();
  *                    return [2 /*return* /, json];
  *             }
  *         });
  *     });
  * }
  * ->
  * async function asyncAwait() {
  *   console.log('Before sleep');
  *   await sleep(1000);
  *   const result = await fetch('')
  *   const json = await result.json();
  *   return json;
  * }
  *
  * @example
  * function generator() {
  *   return __generator(this, function (_a) {
  *   switch (_a.label) {
  *     case 0: return [4 /*yield* /, 1];
  *       case 1:
  *         _a.sent();
  *         return [4 /*yield* /, 2];
  *       case 2:
  *         _a.sent();
  *         return [4 /*yield* /, 3];
  *       case 3:
  *         _a.sent();
  *         return [2 /*return* /];
  *     }
  *   });
  * }
  * ->
  * function* generator() {
  *   yield 1;
  *   yield 2;
  *   yield 3;
  * }
  */

/**
 * __generator opcode map:
 * //  0: next(value?)     - Start or resume the generator with the specified value.
 * //  1: throw(error)     - Resume the generator with an exception. If the generator is
 * //                        suspended inside of one or more protected regions, evaluates
 * //                        any intervening finally blocks between the current label and
 * //                        the nearest catch block or function boundary. If uncaught, the
 * //                        exception is thrown to the caller.
 * //  2: return(value?)   - Resume the generator as if with a return. If the generator is
 * //                        suspended inside of one or more protected regions, evaluates any
 * //                        intervening finally blocks.
 * //  3: break(label)     - Jump to the specified label. If the label is outside of the
 * //                        current protected region, evaluates any intervening finally
 * //                        blocks.
 * //  4: yield(value?)    - Yield execution to the caller with an optional value. When
 * //                        resumed, the generator will continue at the next label.
 * //  5: yield*(value)    - Delegates evaluation to the supplied iterator. When
 * //                        delegation completes, the generator will continue at the next
 * //                        label.
 * //  6: catch(error)     - Handles an exception thrown from within the generator body. If
 * //                        the current label is inside of one or more protected regions,
 * //                        evaluates any intervening finally blocks between the current
 * //                        label and the nearest catch block or function boundary. If
 * //                        uncaught, the exception is thrown to the caller.
 * //  7: endfinally       - Ends a finally block, resuming the last instruction prior to
 * //                        entering a finally block.
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    // __generator
    root
        .find(j.BlockStatement, {
            body: (body: any) => {
                return body.some((statement: any) => {
                    return j.match(statement, {
                        type: 'ReturnStatement',
                        argument: {
                            type: 'CallExpression',
                            callee: {
                                type: 'Identifier',
                                name: '__generator',
                            },
                            arguments: [
                                { type: 'ThisExpression' as const },
                                {
                                    type: 'FunctionExpression' as const,
                                    // @ts-expect-error
                                    params: [{ type: 'Identifier' }],
                                },
                            ],
                        },
                    })
                })
            },
        })
        .forEach((path) => {
            if (!j.FunctionDeclaration.check(path.parent.node)
             && !j.FunctionExpression.check(path.parent.node)) return

            // find the switch statement in __generator
            // collect all the return statements in cases
            // map it to appropriate statements based on
            // the opcode
            // replace the switch statement with the new statements

            const returnStatement = path.node.body.find(statement => j.ReturnStatement.check(statement)) as ReturnStatement
            const generatorCallExpression = returnStatement.argument as CallExpression
            const generatorFunctionExpression = generatorCallExpression.arguments[1] as FunctionExpression
            const generatorStateName = (generatorFunctionExpression.params[0] as Identifier).name
            const switchStatement = generatorFunctionExpression.body.body[0] as SwitchStatement
            const cases = switchStatement.cases

            /* A stack of Protected Regions, which are 4-tuples that describe the labels that make up a try..catch..finally block. */
            const trysList: Array<[
                number | null,
                number | null,
                number | null,
                number | null,
            ]> = []

            /**
             * @example
             * statements = [
             *  [0, [statement1, statement2]],
             *  [1, [statement1]],
             *  ...
             * ]
             */
            const statementsList: any[][] = []
            cases.forEach((caseStatement) => {
                if (!j.Literal.check(caseStatement.test) || typeof caseStatement.test.value !== 'number') return

                const index = caseStatement.test.value
                if (!statementsList[index]) statementsList[index] = []
                const currentStatements = statementsList[index]
                const previousStatements = statementsList[index - 1] || []

                caseStatement.consequent.forEach((statement) => {
                    // __state.trys.push([0, 1, 3, 4]);
                    const isStateTrys = j.match(statement, {
                        type: 'ExpressionStatement',
                        expression: {
                            type: 'CallExpression',
                            callee: {
                                type: 'MemberExpression',
                                object: {
                                    type: 'MemberExpression',
                                    object: {
                                        type: 'Identifier',
                                        name: generatorStateName,
                                    },
                                    property: {
                                        type: 'Identifier',
                                        name: 'trys',
                                    },
                                },
                                property: {
                                    type: 'Identifier',
                                    name: 'push',
                                },
                            },
                            // @ts-expect-error
                            arguments: (args: any[]) => args.length === 1 && j.ArrayExpression.check(args[0]) && args[0].elements.length === 4,
                        },
                    })
                    if (isStateTrys) {
                        const arrayExpression = ((statement as ExpressionStatement).expression as CallExpression).arguments[0] as ArrayExpression
                        const trysValue = arrayExpression.elements.map((element: any) => element?.value as number | null) as any
                        trysList.push(trysValue)
                        return
                    }

                    if (!j.ReturnStatement.check(statement)) {
                        // `_a.label = 1;`
                        // not sure about the use case for this
                        // let's skip it first
                        const stateLabelAssignment = j.match(statement, {
                            type: 'ExpressionStatement',
                            // @ts-expect-error
                            expression: {
                                type: 'AssignmentExpression',
                                operator: '=',
                                left: {
                                    type: 'MemberExpression',
                                    object: {
                                        type: 'Identifier',
                                        name: generatorStateName,
                                    },
                                    property: {
                                        type: 'Identifier',
                                        name: 'label',
                                    },
                                },
                            },
                        })
                        if (stateLabelAssignment) {
                            return
                        }

                        const stateSentCall = j(statement).find(j.CallExpression, {
                            callee: {
                                type: 'MemberExpression',
                                object: {
                                    type: 'Identifier',
                                    name: generatorStateName,
                                },
                                property: {
                                    type: 'Identifier',
                                    name: 'sent',
                                },
                            },
                        })
                        if (stateSentCall.length > 0) {
                            console.log('stateSentCall', index, j(statement).toSource())
                            // if the statement parent is ExpressionStatement
                            // means no one is using the return value
                            // `_a.sent()`
                            if (j.ExpressionStatement.check(statement)
                            && stateSentCall.length === 1
                            && statement.expression === stateSentCall.get().node) {
                                return
                            }

                            // if this is in the catch block
                            // replace the state.sent() call to Identifier('error')
                            const isCatchBlock = trysList.some(trys => trys[1] === index)
                            if (isCatchBlock) {
                                console.log('catch block', j(statement).toSource())
                                stateSentCall.replaceWith(j.identifier('error'))
                                currentStatements.push(statement)
                                return
                            }

                            // replace the state.sent() call with the return value
                            // the value will be get from the last item in the
                            // statements array
                            const lastStatement = previousStatements.at(-1)
                            // It should be the yield statement
                            if (j.ExpressionStatement.check(lastStatement)
                            && j.YieldExpression.check(lastStatement.expression)) {
                                const yieldExpression = lastStatement.expression as YieldExpression
                                const argument = yieldExpression.argument as ExpressionKind
                                const yieldArgument = j.yieldExpression(argument)
                                stateSentCall.replaceWith(yieldArgument)
                                previousStatements.pop()
                                console.log('replaced', j(lastStatement).toSource())
                                if (j.ExpressionStatement.check(statement)) {
                                    currentStatements.push(j.expressionStatement(statement.expression))
                                }
                                else {
                                    currentStatements.push(statement)
                                }
                                return
                            }
                        }

                        currentStatements.push(statement)
                        return
                    }

                    if (j.match(statement, {
                        type: 'ReturnStatement',
                        argument: {
                            type: 'ArrayExpression',
                            // @ts-expect-error
                            elements: elements => elements.length >= 1 && j.Literal.check(elements[0]) && typeof elements[0].value === 'number',
                        },
                    })) {
                        const returnStatement = statement as ReturnStatement
                        const arrayExpression = returnStatement.argument as ArrayExpression
                        const opcode = (arrayExpression.elements[0] as Literal).value
                        const argument = arrayExpression.elements[1] || null as ExpressionKind | null

                        switch (opcode) {
                            case 0:
                                currentStatements.push(j.expressionStatement(j.awaitExpression(argument)))
                                break
                            case 1:
                                currentStatements.push(j.throwStatement(argument))
                                break
                            case 2:
                                argument && currentStatements.push(j.returnStatement(argument))
                                break
                            case 3:
                                // currentStatements.push(j.breakStatement(argument))
                                break
                            case 4:
                                currentStatements.push(j.expressionStatement(j.yieldExpression(argument)))
                                break
                            case 5:
                                currentStatements.push(j.expressionStatement(j.yieldExpression(j.yieldExpression(argument))))
                                break
                            case 6:
                                currentStatements.push(j.throwStatement(argument))
                                break
                            case 7:
                                // currentStatements.push(j.expressionStatement(j.identifier('endfinally')))
                                break
                            default:
                                currentStatements.push(statement)
                        }
                    }
                })
            })

            if (statementsList.length === 0) return

            path.parent.node.generator = true

            const mappedStatements: any[] = []
            // map statements based on the trysList
            statementsList.forEach((statements, index) => {
                const trys = trysList.find(trys => trys.includes(index))
                if (trys) {
                    if (trys[0] === index) {
                        const [tryStart, catchStart, finallyStart, next] = trys
                        if (tryStart === null) return

                        // end should always be catch or finally, single try block is impossible
                        const tryEnd = catchStart !== null ? catchStart : finallyStart
                        if (tryEnd === null) return

                        const tryStatements = statementsList.slice(tryStart, tryEnd).flat()
                        const tryBlock = j.blockStatement(tryStatements)

                        const catchEnd = finallyStart !== null ? finallyStart : next
                        const catchStatements = catchStart !== null && catchEnd !== null
                            ? statementsList.slice(catchStart, catchEnd).flat()
                            : []
                        const catchClause = catchStatements.length > 0
                            ? j.catchClause(j.identifier('error'), null, j.blockStatement(catchStatements))
                            : null

                        const finallyEnd = next
                        const finallyStatements = finallyStart !== null && finallyEnd !== null
                            ? statementsList.slice(finallyStart, finallyEnd).flat()
                            : []
                        const finallyBlock = finallyStatements.length > 0
                            ? j.blockStatement(finallyStatements)
                            : null

                        const tryStatement = j.tryStatement(tryBlock, catchClause, finallyBlock)
                        mappedStatements.push(tryStatement)
                    }
                }
                else {
                    mappedStatements.push(...statements)
                }
            })

            const returnStatementIndex = path.node.body.findIndex((statement) => {
                return j.ReturnStatement.check(statement)
            })
            path.node.body.splice(returnStatementIndex, 1, ...mappedStatements)
        })
}

export default wrap(transformAST)
