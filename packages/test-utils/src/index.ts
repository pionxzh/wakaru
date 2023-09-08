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
export function defineInlineTest(transforms: Transform | Transform[]) {
    const reducedTransform: Transform = Array.isArray(transforms)
        ? (fileInfo, api, options) => {
                let code = fileInfo.source
                for (const transform of transforms) {
                    const newFileInfo = { ...fileInfo, source: code }
                    const newResult = transform(newFileInfo, api, options)
                    if (newResult) code = newResult
                }
                return code
            }
        : transforms

    function _inlineTest(
        modifier: 'skip' | 'only' | null,
        testName: string,
        input: string,
        expectedOutput: string,
    ) {
        const itFn = modifier ? it[modifier] : it
        itFn(testName, () => {
            try {
                runInlineTest(reducedTransform, {}, {
                    source: input,
                }, expectedOutput)
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
        skip: InlineTest
        only: InlineTest
    }

    inlineTest.skip = _inlineTest.bind(null, 'skip')
    inlineTest.only = _inlineTest.bind(null, 'only')

    return inlineTest
}
