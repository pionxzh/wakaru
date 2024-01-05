import { mergeTransformationRule } from '@wakaru/shared/rule'
import { runInlineTest } from 'jscodeshift/src/testUtils'
import { it } from 'vitest'
import type { TransformationRule } from '@wakaru/shared/rule'

export function defineInlineTestWithOptions(rule: TransformationRule) {
    return function inlineTest(
        testName: string,
        options: any,
        input: string,
        expectedOutput: string,
    ) {
        it(testName, () => {
            runInlineTest(
                rule.toJSCodeshiftTransform(),
                options,
                { source: input },
                expectedOutput,
                { parser: 'babylon' },
            )
        })
    }
}

type InlineTest = (
    testName: string,
    input: string,
    expectedOutput: string,
) => void

/**
 * Wrapper around `jscodeshift`'s `runInlineTest` that allows for a more
 * declarative syntax.
 *
 * - Supports multiple transforms
 * - Supports `skip` and `only` modifiers
 */
export function defineInlineTest(rules: TransformationRule | TransformationRule[]) {
    const mergedTransform = Array.isArray(rules)
        ? mergeTransformationRule(rules.map(rule => rule.name).join(' + '), rules).toJSCodeshiftTransform()
        : rules.toJSCodeshiftTransform()

    function _inlineTest(
        modifier: 'skip' | 'only' | 'todo' | 'fails' | null,
        testName: string,
        input: string,
        expectedOutput: string,
    ) {
        const itFn = modifier ? it[modifier] : it
        itFn(testName, () => {
            try {
                runInlineTest(
                    mergedTransform,
                    {},
                    { source: input },
                    expectedOutput,
                    { parser: 'babylon' },
                )
            }
            catch (err) {
                /**
                 * Prevent test utils from showing up in the stack trace.
                 */
                if (err instanceof Error && err.stack) {
                    const stack = err.stack
                    const stacks = stack.split('\n')
                    const newStacks = stacks.filter((line) => {
                        const blockList = [
                            // /@vitest\/runner/,
                            /test-utils\\src\\index\.ts/,
                            /jscodeshift\\src\\testUtils\.js/,
                        ]
                        return !blockList.some(regex => regex.test(line))
                    })
                    err.stack = newStacks.join('\n')
                }
                throw err
            }
        })
    }

    const inlineTest = _inlineTest.bind(null, null) as InlineTest & {
        /**
         * Use `.skip` to skip a test in a given suite. Consider using `.todo` or `.fixme` instead
         * if those are more appropriate.
         */
        skip: InlineTest
        /**
         * Use `.only` to only run certain tests in a given suite. This is useful when debugging.
         */
        only: InlineTest
        /**
         * Use `.todo` when you are writing a test but the feature you are testing is not yet
         * implemented.
         */
        todo: InlineTest
        /**
         * Use `.fixme` when you are writing a test and **expecting** it to fail.
         */
        fixme: InlineTest
    }

    inlineTest.skip = _inlineTest.bind(null, 'skip')
    inlineTest.only = _inlineTest.bind(null, 'only')
    inlineTest.todo = _inlineTest.bind(null, 'todo')
    inlineTest.fixme = _inlineTest.bind(null, 'fails')

    return inlineTest
}
