import jscodeshift from 'jscodeshift'

// @ts-expect-error - no types
import getParser from 'jscodeshift/src/getParser'
import { transformationMap } from './transformations'
import { arraify } from './utils/arraify'
import type { MaybeArray } from './utils/types'
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
