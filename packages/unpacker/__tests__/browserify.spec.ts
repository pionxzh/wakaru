import { readFile } from 'node:fs/promises'
import path from 'node:path'
import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'
import { describe, expect, it } from 'vitest'
import { unpack } from '../src'

const format = (code: string) => prettier.format(code, {
  parser: 'babel',
  plugins: [babelParser],
})

describe('Browserify', () => {
  it('testcases/browserify', async () => {
    const sourcePath = path.resolve(__dirname, '../../../testcases/browserify/dist/index.js')
    const source = await readFile(sourcePath, 'utf8')
    const result = unpack(source)
    if (!result) throw new Error('Failed to unpack')

    expect(result.moduleIdMapping).toMatchSnapshot()

    expect(result.modules.length).toBe(4)

    const modules = result.modules.map(({ id, isEntry, code }) => ({ id, isEntry, code: format(code) }))
    expect(modules).toMatchSnapshot()
  })
})
