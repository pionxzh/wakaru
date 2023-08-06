import fs from 'node:fs/promises'
import * as globby from 'globby'
import babelParser from 'prettier/parser-babel'
import prettier from 'prettier/standalone'

const resolvedPaths = globby.sync([
    'testcases/**/dist/*.js',
    '!testcases/**/dist/*.pretty.js',
    '!**/node_modules/**',
])

resolvedPaths.forEach(async (sourcePath) => {
    // dist/index.js -> dist/index.pretty.js
    const targetPath = sourcePath.replace(/\.js$/, '.pretty.js')

    console.log(`Processing: ${sourcePath} -> ${targetPath}`)
    const code = await fs.readFile(sourcePath, 'utf-8')

    const result = prettier.format(code, {
        parser: 'babel',
        plugins: [babelParser],
    })

    await fs.writeFile(targetPath, result)
})
