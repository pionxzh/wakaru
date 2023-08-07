import { runInlineTest } from 'jscodeshift/src/testUtils'
import { it } from 'vitest'
import type { Transform } from 'jscodeshift'

export function defineInlineTestWithOptions(transform: Transform) {
    return function inlineTest(
        testName: string,
        options: any,
        input: string,
        expectedOutput: string,
    ) {
        it(testName, () => {
            runInlineTest(transform, options, {
                source: input,
            }, expectedOutput)
        })
    }
}

export function defineInlineTest(transform: Transform) {
    return function inlineTest(
        testName: string,
        input: string,
        expectedOutput: string,
    ) {
        it(testName, () => {
            runInlineTest(transform, {}, {
                source: input,
            }, expectedOutput)
        })
    }
}
