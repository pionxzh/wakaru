import { expect, it } from 'vitest'
import { pathCompletion } from '../src/path'

const baseDir = __dirname

interface CustomMatchers<R = unknown> {
    toBeSamePath(expected: R): R
}

declare module 'vitest' {
    interface Assertion<T = any> extends CustomMatchers<T> {}
    interface AsymmetricMatchersContaining extends CustomMatchers {}
}

expect.extend({
    // Check if the path is the same with automatic path separator conversion
    toBeSamePath: (received, expected) => {
        const receivedPath = received.replace(/\\/g, '/')
        const expectedPath = expected.replace(/\\/g, '/')
        const pass = receivedPath === expectedPath
        return {
            pass,
            message: () => `Expected ${receivedPath} ${pass ? 'not ' : ''}to be same path as ${expectedPath}`,
            actual: receivedPath,
            expected: expectedPath,
        }
    },
})

it('pathCompletion', () => {
    expect(pathCompletion({ input: '', baseDir })).toBeSamePath('./folder/')
    expect(pathCompletion({ input: 'f', baseDir })).toBeSamePath('./folder/')
    expect(pathCompletion({ input: 'fold', baseDir })).toBeSamePath('./folder/')
    expect(pathCompletion({ input: 'folder', baseDir })).toBeSamePath('./folder/')
    expect(pathCompletion({ input: 'folder/', baseDir })).toBeSamePath('./folder/')
    expect(pathCompletion({ input: './folder', baseDir })).toBeSamePath('./folder/')
    expect(pathCompletion({ input: './folder/', baseDir })).toBeSamePath('./folder/')

    expect(pathCompletion({ input: 'folder/f', baseDir })).toBeSamePath('./folder/file1.js')
    expect(pathCompletion({ input: 'folder/file1', baseDir })).toBeSamePath('./folder/file1.js')
    expect(pathCompletion({ input: 'folder/file1.js', baseDir })).toBeSamePath('./folder/file1.js')
    expect(pathCompletion({ input: './folder/file1', baseDir })).toBeSamePath('./folder/file1.js')

    expect(pathCompletion({ input: 'folder/file12', baseDir })).toBeSamePath('./folder/file12.js')
    expect(pathCompletion({ input: 'folder/file12.js', baseDir })).toBeSamePath('./folder/file12.js')

    expect(pathCompletion({ input: 'folder/file123', baseDir })).toBeSamePath('./folder/file123.js')

    expect(pathCompletion({ input: 'folder/nested', baseDir })).toBeSamePath('./folder/nested/')
})
