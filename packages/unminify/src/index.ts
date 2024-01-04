import { api } from '@wakaru/shared/jscodeshift'
import { transformationRules } from './transformations'
import { arraify } from './utils/arraify'
import type { MaybeArray } from './utils/arraify'
import type { FileInfo, Transform } from 'jscodeshift'

export * from './transformations'

export function runDefaultTransformation<P extends Record<string, any>>(
    fileInfo: FileInfo,
    params: P = {} as any,
) {
    const transforms = transformationRules.map(rule => rule.toJSCodeshiftTransform())
    return runTransformations(fileInfo, transforms, params)
}

export function runTransformationIds<P extends Record<string, any>>(
    fileInfo: FileInfo,
    ids: string[],
    params: P = {} as any,
) {
    const transforms = ids.map(id => transformationRules.find(rule => rule.id === id)?.toJSCodeshiftTransform()).filter(Boolean) as Transform[]
    return runTransformations(fileInfo, transforms, params)
}

export function runTransformations<P extends Record<string, any>>(
    fileInfo: FileInfo,
    transforms: MaybeArray<Transform>,
    params: P = {} as any,
) {
    const { path } = fileInfo

    const transformFns = arraify(transforms)
    let code = fileInfo.source
    for (const transform of transformFns) {
        try {
            const newResult = transform({ path, source: code }, api, params)
            if (newResult) code = newResult
        }
        catch (err: any) {
            console.error(`\nError running transformation ${transform.name} on ${path}`, err)

            printSourceWithErrorLoc(err, code)

            break
        }
    }

    return {
        path,
        code,
    }
}
