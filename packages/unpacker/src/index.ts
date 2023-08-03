import path from 'node:path'
import fs from 'node:fs/promises'
import type { ArrowFunctionExpression, ClassDeclaration, Collection, FunctionDeclaration, FunctionExpression, Identifier, JSCodeshift, Literal, ObjectProperty, VariableDeclaration } from 'jscodeshift'
import jscodeshift from 'jscodeshift'
// @ts-expect-error - no types
import getParser from 'jscodeshift/src/getParser'
import { isTopLevel, prettierFormat, pruneComments } from './utils'
import type { Module } from './Module'
import { getModules as getModulesForWebpack5 } from './extractors/webpack5'
import { getModules as getModulesForWebpack4 } from './extractors/webpack4'

export async function unpack() {
    // const input = path.resolve('../../testcases/webpack/dist/index.js')
    const input = path.resolve('../../wb.js')
    const code = await fs.readFile(input, 'utf-8')
    const parser = getParser()
    const j = jscodeshift.withParser(parser)
    const root = j(code)

    pruneComments(j, root)

    const modules: Set<Module> | null
       = getModulesForWebpack5(j, root)
      || getModulesForWebpack4(j, root)

    if (!modules) {
        console.error('Failed to locate modules')
        return
    }

    // const output = path.resolve('../../testcases/webpack/dist/output.js')
    // const formattedCode = prettierFormat(root.toSource())
    // await fs.writeFile(output, formattedCode, 'utf-8')

    // write modules to file
    const modulesOutput = path.resolve('../../testcases/webpack/dist/modules.js')
    const modulesCode = Array.from(modules)
        .map(module => `\n\n/**** ${module.id} ****/\n\n${prettierFormat(module.ast.toSource())}`).join('\n')
    await fs.writeFile(modulesOutput, modulesCode, 'utf-8')
}

unpack()
