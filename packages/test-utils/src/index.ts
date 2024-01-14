import { executeTransformationRules } from '@wakaru/shared/runner'
import { expect, it } from 'vitest'
import type { TransformationRule } from '@wakaru/shared/rule'

interface InlineTest {
    (testName: string, input: string, expected: string): void
}

type TestModifier = 'skip' | 'only' | 'todo' | 'fails'

export function defineInlineTest(rules: TransformationRule | TransformationRule[]) {
    function _inlineTest(
        modifier: TestModifier | null,
        options: any,
        testName: string,
        input: string,
        expected: string,
    ) {
        const itFn = modifier ? it[modifier] : it
        if (!itFn) throw new Error(`Unknown modifier "${modifier}" for test: ${testName}`)

        /**
         * Capture the stack trace of the test function call so that we can show it in the error
         */
        const _error = new Error('_')
        const testCallStack = _error.stack ? _error.stack.split('\n')[2] : null

        itFn(testName, async () => {
            try {
                const output = await executeTransformationRules(input, 'test.js', rules, options)
                expect(output.code.trim().replace(/\r\n/g, '\n'))
                    .toEqual(expected.trim().replace(/\r\n/g, '\n'))
            }
            catch (err) {
                /**
                 * Prevent test utils from showing up in the stack trace.
                 */
                if (err instanceof Error && err.stack) {
                    const stacks = err.stack.split('\n')
                    const newStacks = [testCallStack, ...stacks]
                    err.stack = newStacks.join('\n')
                }
                throw err
            }
        })
    }

    const createModifiedFunction = (modifier: TestModifier | null, options: any = {}): InlineTest => {
        return Object.assign(
            _inlineTest.bind(null, modifier, options),
            {
                withOptions: (newOptions: any) => {
                    return createModifiedFunction(modifier, newOptions)
                },
            },
        )
    }

    const inlineTest = createModifiedFunction(null) as InlineTest & {
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
        /**
         * Use `.withOptions` to pass options to the transformation rules.
         */
        withOptions: (options: any) => InlineTest
    }

    inlineTest.skip = createModifiedFunction('skip')
    inlineTest.only = createModifiedFunction('only')
    inlineTest.todo = createModifiedFunction('todo')
    inlineTest.fixme = createModifiedFunction('fails')
    inlineTest.withOptions = options => _inlineTest.bind(null, null, options)

    return inlineTest
}
