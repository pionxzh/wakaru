declare module 'lebab' {
    export type LebabRule =
    | 'class'
    | 'template'
    | 'arrow'
    | 'arrow-return'
    | 'let'
    | 'default-param'
    | 'destruct-param'
    | 'arg-spread'
    | 'arg-rest'
    | 'obj-method'
    | 'obj-shorthand'
    | 'no-strict'
    | 'commonjs'
    | 'exponent'
    | 'multi-var'
    | 'for-of'
    | 'for-each'
    | 'includes'

    export interface TransformWarning {
        line: number
        msg: string
        type: string
    }

    export interface TransformResult {
        code: string
        warnings: TransformWarning[]
    }

    export function transform(
        input: string,
        transformRules: LebabRule[]
    ): TransformResult
}
