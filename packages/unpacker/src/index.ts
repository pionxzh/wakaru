import fs from 'node:fs/promises'
import path from 'node:path'
import jscodeshift from 'jscodeshift'

// @ts-expect-error - no types
import getParser from 'jscodeshift/src/getParser'

import { getModulesFromBrowserify } from './extractors/browserify'
import { getModulesFromWebpack } from './extractors/webpack'
import { prettierFormat } from './utils'

export async function unpack() {
    const inputPath = process.argv[2]
    const code = await fs.readFile(inputPath, 'utf-8')
    const parser = getParser()
    const j = jscodeshift.withParser(parser)
    const root = j(code)

    const result
     = getModulesFromWebpack(j, root)
    || getModulesFromBrowserify(j, root)

    if (!result) {
        console.error('Failed to locate modules')
        return
    }

    const { modules, moduleIdMapping } = result

    // write modules to file
    const modulesOutput = path.resolve('preview.js')
    const modulesCode = Array.from(modules)
        .map((module) => {
            const moduleId = moduleIdMapping.get(module.id) ?? module.id
            const entryMark = module.isEntry ? ' (entry)' : ''
            return `\n\n/**** ${moduleId}${entryMark} ****/\n\n${prettierFormat(module.ast.toSource())}`
        }).join('\n')
    await fs.writeFile(modulesOutput, modulesCode, 'utf-8')
}

unpack()
