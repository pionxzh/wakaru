import { readFile } from 'node:fs/promises'
import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'
import { describe, expect, it } from 'vitest'
import { unpack } from '../src'

const format = (code: string) => prettier.format(code, {
    parser: 'babel',
    plugins: [babelParser],
})

describe('Webpack 4', () => {
    it('testcases/webpack4', async () => {
        const source = await readFile('../../testcases/webpack4/dist/index.js', 'utf8')
        const result = unpack(source)
        if (!result) throw new Error('Failed to unpack')

        expect(result.moduleIdMapping).toMatchSnapshot()

        expect(result.modules.length).toBe(52)

        const modules = result.modules.map(({ id, isEntry, code }) => ({ id, isEntry, code: format(code) }))
        expect(modules).toMatchSnapshot()
    })
})
