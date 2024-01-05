import type { JSCodeshiftTransformationRule } from './jscodeshiftRule'
import type { StringTransformationRule } from './stringRule'
import type { ModuleMapping, ModuleMeta } from './types'
import type { Transform } from 'jscodeshift'
import type { ZodSchema } from 'zod'

export * from './jscodeshiftRule'
export * from './stringRule'

export interface SharedParams {
    moduleMapping?: ModuleMapping
    moduleMeta?: ModuleMeta
}

export interface BaseTransformationRule {
    type: 'jscodeshift' | 'string' | 'rule-set'
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
    toJSCodeshiftTransform(): Transform
}

export type TransformationRule<Schema extends ZodSchema = ZodSchema> =
    | JSCodeshiftTransformationRule<Schema>
    | StringTransformationRule<Schema>
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

    toJSCodeshiftTransform(): Transform {
        const rules = this.rules
        return function mergedTransform(file, api, options) {
            let source = file.source
            for (const rule of rules) {
                const transform = rule.toJSCodeshiftTransform()
                const newResult = transform({ ...file, source }, api, options)
                if (newResult) source = newResult
            }
            return source
        }
    }
}

export function mergeTransformationRule(name: string, rules: TransformationRule[]): MergedTransformationRule {
    return new MergedTransformationRule({ name, rules })
}
