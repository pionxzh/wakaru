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

export function defineInlineTest(transforms: Transform | Transform[]) {
    function inlineTest(
        testName: string,
        input: string,
        expectedOutput: string,
    ) {
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

        it(testName, () => {
            try {
                runInlineTest(reducedTransform, {}, {
                    source: input,
                }, expectedOutput)
            }
            catch (err) {
                if (err instanceof Error && err.stack) {
                    const stack = err.stack
                    const stacks = stack.split('\n')
                    const newStacks = stacks.filter((line) => {
                        const blockList = [
                            // /@vitest\/runner/,
                            /test-utils\.ts/,
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

    inlineTest.skip = function skipInlineTest(
        testName: string,
        _input: string,
        _expectedOutput: string,
    ) {
        it.skip(testName, () => {})
    }

    return inlineTest
}
