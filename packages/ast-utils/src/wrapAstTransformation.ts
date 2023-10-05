import type { Core, JSCodeshift, Options, Transform } from 'jscodeshift'

export interface Context {
    root: ReturnType<Core>
    j: JSCodeshift
    filename: string
}

export interface ASTTransformation<Params = unknown> {
    (context: Context, params: Params): string | void
}

function astTransformationToJSCodeshiftModule<Params extends Options>(
    transformAST: ASTTransformation<Params>,
): Transform {
    // @ts-expect-error - jscodeshift is not happy
    const transform: Transform = (file, api, options: Params) => {
        const j = api.jscodeshift
        const root = j(file.source)
        const result = transformAST({ root, j, filename: file.path }, options)
        return result ?? root.toSource({ lineTerminator: '\n' })
    }

    return transform
}

export const wrapAstTransformation = astTransformationToJSCodeshiftModule
