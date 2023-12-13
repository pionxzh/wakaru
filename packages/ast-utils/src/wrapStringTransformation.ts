import type { Options, Transform } from 'jscodeshift'

export interface StringTransformation<Params = object> {
    (code: string, params: Params): string | void
}

export function wrapStringTransformation<Params extends Options>(
    transformAST: StringTransformation<Params>,
): Transform {
    // @ts-expect-error - jscodeshift is not happy
    const transform: Transform = (file, api, options: Params) => {
        const code = file.source
        const result = transformAST(code, options)
        return result ?? code
    }

    return transform
}
