import { nonNullable } from '@wakaru/shared/array'
import { executeTransformationRules } from '@wakaru/shared/runner'
import { transformationRules } from './transformations/node'
import type { FileInfo } from 'jscodeshift'

export { transformationRules, transformationRuleIds } from './transformations'

export function runDefaultTransformationRules<P extends Record<string, any>>(
    fileInfo: FileInfo,
    params: P = {} as any,
) {
    return executeTransformationRules(fileInfo.source, fileInfo.path, transformationRules, params)
}

export function runTransformationRules<P extends Record<string, any>>(
    fileInfo: FileInfo,
    ruleIds: string[],
    params: P = {} as any,
) {
    const rules = ruleIds.map(id => transformationRules.find(rule => rule.id === id)).filter(nonNullable)
    return executeTransformationRules(fileInfo.source, fileInfo.path, rules, params)
}
