import jscodeshift from 'jscodeshift'

import { transformationMap } from './transformations'
import { arraify } from './utils/arraify'
import type { MaybeArray } from './utils/arraify'
import type { FileInfo, Transform } from 'jscodeshift'

export function runDefaultTransformation<P extends Record<string, any>>(fileInfo: FileInfo, params: P = {} as any) {
    const transforms = Object.values(transformationMap)
    return runTransformations(fileInfo, transforms, params)
}

export function runTransformations<P extends Record<string, any>>(
    fileInfo: FileInfo,
    transforms: MaybeArray<Transform>,
    params: P = {} as any,
) {
    const { path } = fileInfo

    const j = jscodeshift.withParser('babylon')
    const api = {
        j,
        jscodeshift: j,
        stats: () => {},
        report: () => {},
    }

    const transformFns = arraify(transforms)
    let code = fileInfo.source
    for (const transform of transformFns) {
        try {
            const newResult = transform({ path, source: code }, api, params)
            if (newResult) code = newResult
        }
        catch (err: any) {
            if ('loc' in err) {
                console.error(err)

                const padLeft = (str: string, len: number, char: string) => {
                    const count = len > str.length ? len - str.length : 0
                    return `${char.repeat(count)}${str}`
                }
                function printLine(line: number, column?: number) {
                    const lines = code.split('\n')
                    const lineNumber = padLeft(line.toString(), 5, ' ')
                    const lineContent = lines[line - 1]
                    const linePrefix = `${lineNumber} | `
                    console.error(linePrefix + lineContent)

                    if (column !== undefined) {
                        const linePointer = `${' '.repeat(linePrefix.length + column - 1)}^`
                        console.error(linePointer)
                    }
                }

                const loc: any = err.loc
                printLine(loc.line - 2)
                printLine(loc.line - 1)
                printLine(loc.line, loc.column)
                printLine(loc.line + 1)
                printLine(loc.line + 2)
            }
            else {
                console.error(err)
            }

            break
        }
    }

    return {
        path,
        code,
    }
}

export { transformationMap } from './transformations'
