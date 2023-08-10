import { readFile } from 'node:fs/promises'
import { describe, expect, it } from 'vitest'
import { unpack } from '../src'

describe('Browserify', () => {
    it('testcases/browserify', async () => {
        const source = await readFile('../../testcases/browserify/dist/index.js', 'utf8')
        const result = unpack(source)
        if (!result) throw new Error('Failed to unpack')

        expect(result.moduleIdMapping).toMatchSnapshot()

        expect(result.modules.length).toBe(4)

        const modules = [...result.modules.values()]
            .map(({ id, isEntry, code }) => ({ id, isEntry, code }))
        expect(modules).toMatchSnapshot()
    })
})
