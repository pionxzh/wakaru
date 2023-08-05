import * as path from 'node:path'
import fsa from 'fs-extra'
import * as globby from 'globby'
import jscodeshift from 'jscodeshift'

// @ts-expect-error - no types
import getParser from 'jscodeshift/src/getParser'
import { transformationMap } from './transformations'
import { arraify } from './utils/arraify'
import type { MaybeArray } from './utils/types'
import type { FileInfo, Transform } from 'jscodeshift'

export async function codemod(
    paths: string[],
    output: string,
) {
    const cwd = process.cwd()
    const resolvedPaths = globby.sync(paths.concat('!node_modules'))
    const outputPaths: string[] = []
    const outputDir = path.resolve(cwd, output)
    fsa.ensureDirSync(outputDir)

    resolvedPaths.forEach(async (p) => {
        const source = fsa
            .readFileSync(p)
            .toString()
            .split('\r\n')
            .join('\n')

        const fileInfo: FileInfo = {
            path: p,
            source,
        }
        const result = runDefaultTransformation(fileInfo)

        if (source !== result.code) {
            console.log(`Writing file: ${p}`)
            const outputPath = path.join(outputDir, path.relative(cwd, p))
            outputPaths.push(outputPath)
            fsa.ensureDirSync(path.dirname(outputPath))
            fsa.writeFileSync(outputPath, result.code)
        }
    })
}

export function runDefaultTransformation(fileInfo: FileInfo, params: object = {}) {
    const transforms = Object.values(transformationMap)
    return runTransformations(fileInfo, transforms, params)
}

export function runTransformations(
    fileInfo: FileInfo,
    transforms: MaybeArray<Transform>,
    params: object = {},
) {
    const { path } = fileInfo

    const parser = getParser()
    const j = jscodeshift.withParser(parser)
    const api = {
        j,
        jscodeshift: j,
        stats: () => {},
        report: () => {},
    }

    let changed = false

    const transformFns = arraify(transforms)
    const result = transformFns.reduce<string>((code, transform) => {
        const result = transform({ path, source: code }, api, params)
        changed = changed || (!!result && result !== code)
        return result ?? code
    }, fileInfo.source)

    return {
        path,
        code: result,
        skipped: !changed,
    }
}

export { transformationMap } from './transformations'
