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

    withId(id: string): BaseTransformationRule
}

export interface BaseTransformationRuleSet {
    rules: BaseTransformationRule[]
    getRuleByName(name: string): BaseTransformationRule | undefined
}

export function mergeTransformationRule(name: string, rules: BaseTransformationRule[]): BaseTransformationRule {
    return {
        id: name,
        name,
        tags: rules.flatMap(r => r.tags),
        toJSCodeshiftTransform() {
            return function mergedTransform(file, api, options) {
                let source = file.source
                for (const rule of rules) {
                    const transform = rule.toJSCodeshiftTransform()
                    const newResult = transform({ ...file, source }, api, options)
                    if (newResult) source = newResult
                }
                return source
            }
        },
        withId(_id: string) {
            throw new Error('Not implemented')
        },
    }
}
