import { readFile } from 'node:fs/promises'
import { describe, expect, it } from 'vitest'
import { unpack } from '../src'

describe('Webpack 5', () => {
    it('testcases/webpack5', async () => {
        const source = await readFile('../../testcases/webpack5/dist/index.js', 'utf8')
        const result = unpack(source)
        if (!result) throw new Error('Failed to unpack')

        expect(result.moduleIdMapping).toMatchSnapshot()

        expect(result.modules.size).toBe(5)

        const modules = [...result.modules.values()]
            .map(({ id, isEntry, code }) => ({ id, isEntry, code }))
        expect(modules).toMatchSnapshot()
    })
})
