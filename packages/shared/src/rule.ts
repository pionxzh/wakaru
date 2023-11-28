import type { AstGrepTransformationRule } from './astGrepRule'
import type { JSCodeshiftTransformationRule } from './jscodeshiftRule'
import type { StringTransformationRule } from './stringRule'
import type { ModuleMapping, ModuleMeta } from './types'
import type { API, FileInfo, Options } from 'jscodeshift'
import type { ZodSchema } from 'zod'

export * from './astGrepRule'
export * from './jscodeshiftRule'
export * from './stringRule'

export interface SharedParams {
    moduleMapping?: ModuleMapping
    moduleMeta?: ModuleMeta
}

/**
 * `Transform` from `jscodeshift`. The type in `jscodeshift` is not accurate,
 * so we have to re-define it here.
 *
 * Async support has been added in https://github.com/facebook/jscodeshift/pull/237
 */
export interface JSCodeshiftTransform {
    /**
     * If a string is returned and it is different from passed source, the transform is considered to be successful.
     * If a string is returned but it's the same as the source, the transform is considered to be unsuccessful.
     * If nothing is returned, the file is not supposed to be transformed (which is ok).
     */
    (file: FileInfo, api: API, options: Options): Promise<string | null | undefined | void> | string | null | undefined | void
}

export interface BaseTransformationRule {
    type: 'jscodeshift' | 'string' | 'ast-grep' | 'rule-set'
    /**
     * The unique id of the rule
     */
    id: string
    /**
     * Thr name of the rule
     */
    name: string
    /**
     * The tags for the rule
     */
    tags: string[]
    /**
     * The zod schema for the options
     */
    schema?: ZodSchema

    /**
     * convert to jscodeshift compatible transform
     */
    toJSCodeshiftTransform(): JSCodeshiftTransform
}

export type TransformationRule<Schema extends ZodSchema = ZodSchema> =
    | JSCodeshiftTransformationRule<Schema>
    | StringTransformationRule<Schema>
    | AstGrepTransformationRule<Schema>
    | MergedTransformationRule

export class MergedTransformationRule implements BaseTransformationRule {
    type = 'rule-set' as const

    id: string

    name: string

    tags: string[]

    schema?: ZodSchema

    rules: TransformationRule[]

    constructor({
        name,
        tags = [],
        rules,
    }: {
        name: string
        tags?: string[]
        rules: TransformationRule[]
    },
    ) {
        this.id = name
        this.name = name
        this.tags = tags
        this.rules = rules
    }

    toJSCodeshiftTransform(): JSCodeshiftTransform {
        const rules = this.rules
        return async function mergedTransform(file, api, options) {
            let source = file.source
            for (const rule of rules) {
                const transform = rule.toJSCodeshiftTransform()
                const newResult = await transform({ ...file, source }, api, options)
                if (newResult) source = newResult
            }
            return source
        }
    }
}

export function mergeTransformationRule(name: string, rules: TransformationRule[]): MergedTransformationRule {
    return new MergedTransformationRule({ name, rules })
}
