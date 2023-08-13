import wrap from '../wrapAstTransformation'
import type { ASTTransformation, Context } from '../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { ArrayExpression, CallExpression, ExpressionStatement, FunctionExpression, Identifier, Literal, ReturnStatement, SwitchStatement, ThisExpression, YieldExpression } from 'jscodeshift'

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
export const transformAST: ASTTransformation = (context) => {
    transform__generator(context)
    transform__awaiter(context)
}

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

/**
 * ES2015 Generator Helpers: __generator
 *
 * ```js
 * function generatorFn() {
 *  return __generator(thisArg, bodyFn);
 * }
 * ```
 */
export function transform__generator(context: Context) {
    const { root, j } = context

    // https://github.com/microsoft/TypeScript/blob/634d3a1db5c69c1425119a74045790a4d1dc3046/src/compiler/transformers/generators.ts#L161

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
            cases?.length > 0 && cases.forEach((caseStatement) => {
                if (!j.Literal.check(caseStatement.test) || typeof caseStatement.test.value !== 'number') return

                const index = caseStatement.test.value
                if (!statementsList[index]) statementsList[index] = []
                const currentStatements = statementsList[index]
                const previousStatements = statementsList[index - 1] || []

                caseStatement.consequent.forEach((statement) => {
                    /**
                     * __state.trys.push([0, 1, 3, 4]);
                     *
                     * Mark the start of a protected region.
                     * [
                     *   startOfTry,
                     *   startOfCatch,
                     *   startOfFinally,
                     *   nextLabel
                     * ]
                     */
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

                    /**
                     * `__state.label = 1;`
                     * goto the label
                     * but now we don't need it, skip first
                     */
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

                    /**
                     * `__state.sent()`
                     */
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
                        // if the statement parent is ExpressionStatement
                        // means no one is using the return value
                        // it's a empty `__state.sent()`
                        if (j.ExpressionStatement.check(statement)
                            && stateSentCall.length === 1
                            && statement.expression === stateSentCall.get().node) {
                            return
                        }

                        // if this is in the catch block
                        // replace the `__state.sent()` call to Identifier('error')
                        // this will be the error argument in the catch block
                        const isCatchBlock = trysList.some(trys => trys[1] === index)
                        if (isCatchBlock) {
                            stateSentCall.replaceWith(j.identifier('error'))
                        }

                        // replace `__state.sent()` with the return value
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
                            if (j.ExpressionStatement.check(statement)) {
                                currentStatements.push(j.expressionStatement(statement.expression))
                            }
                            else {
                                currentStatements.push(statement)
                            }
                            return
                        }
                    }

                    /**
                     * return [opcode, argument]
                     *
                     * This naive implementation will not work in control flow.
                     * We need to collect all the `__state.label`, `__state.sent()`
                     * and `__state.trys` statements, and the return opcode to
                     * build the control flow graph.
                     *
                     * And then do a data flow analysis to reconstruct the
                     * correct flow.
                     */
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
                        const argument = (arrayExpression.elements[1] || null) as ExpressionKind | null

                        let result: any
                        switch (opcode) {
                            case 0:
                                // next(value?)
                                break
                            case 1:
                                // throw(error)
                                break
                            case 2:
                                // return(value)
                                result = argument ? j.returnStatement(argument) : null
                                break
                            case 3:
                                // break(label)
                                break
                            case 4:
                                // yield(value)
                                result = j.expressionStatement(j.yieldExpression(argument))
                                break
                            case 5:
                                // yield*(value)
                                result = j.expressionStatement(j.yieldExpression(j.yieldExpression(argument)))
                                break
                            case 6:
                                // catch(error)
                                break
                            case 7:
                                // endfinally
                                break
                            default:
                                result = statement
                        }

                        if (result) currentStatements.push(result)
                        return
                    }

                    currentStatements.push(statement)
                })
            })

            path.parent.node.generator = true

            const mappedStatements: any[] = []

            // map statements based on the trysList
            statementsList.forEach((statements, index) => {
                const trys = trysList.find((trys) => {
                    const start = trys[0]
                    const end = trys[3] || trys[2] || trys[1]
                    if (start === null || end === null) return false
                    return index >= start && index < end
                })
                if (trys) {
                    if (trys[0] === index) {
                        const [tryStart = null, catchStart = null, finallyStart = null, next = null] = trys
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
                    // other index in trys list should be ignored
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

/**
 * ES2017 async/await: __awaiter
 *
 * ```js
 * function asyncAwait() {
 *   return __awaiter(thisArg, _arguments, promise, generatorFn)
 * ```
 *
 * Currently arguments and promise are not handled.
 * More samples for the usage of __awaiter are needed.
 */
export function transform__awaiter(context: Context) {
    const { root, j } = context

    root
        .find(j.ReturnStatement, {
            argument: {
                type: 'CallExpression',
                callee: {
                    type: 'Identifier',
                    name: '__awaiter',
                },
                arguments: (args) => {
                    return args.length === 4
                        && j.ThisExpression.check(args[0])
                        && j.FunctionExpression.check(args[3])
                        && args[3].generator === true
                },
            },
        })
        .forEach((path) => {
            if (!j.BlockStatement.check(path.parent.node)) return
            if (!j.FunctionDeclaration.check(path.parent.parent.node)) return

            const callExpression = path.node.argument as CallExpression
            const args = callExpression.arguments as [ThisExpression, unknown, unknown, FunctionExpression]
            const generatorFn = args[3]
            const statements = generatorFn.body.body

            // replace all yield expressions with await expressions
            j(statements).find(j.YieldExpression).replaceWith((path) => {
                return j.awaitExpression(path.node.argument as ExpressionKind)
            })

            // unwrap the __awaiter call and move the statements to the parent function
            // mark the parent function as async
            path.parent.parent.node.async = true
            path.replace(...statements)
        })
}

export default wrap(transformAST)
