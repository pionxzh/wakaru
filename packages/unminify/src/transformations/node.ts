import moduleMapping from './module-mapping'
import moduleMappingGrep from './module-mapping.grep'
import unBoolean from './un-boolean'
import unBooleanGrep from './un-boolean.grep'
import unEsmoduleFlag from './un-esmodule-flag'
import unEsmoduleFlagGrep from './un-esmodule-flag.grep'
import unInfinity from './un-infinity'
import unInfinityGrep from './un-infinity.grep'
import unUndefined from './un-undefined'
import unUndefinedGrep from './un-undefined.grep'
import unUseStrict from './un-use-strict'
import unUseStrictGrep from './un-use-strict.grep'
import { transformationRules as _transformationRules, transformationRuleIds } from './index'
import type { TransformationRule } from '@wakaru/shared/rule'

const ruleAlternativesOnNodejs = new Map<TransformationRule, TransformationRule>([
    [moduleMapping, moduleMappingGrep],
    [unUseStrict, unUseStrictGrep],
    [unEsmoduleFlag, unEsmoduleFlagGrep],
    [unBoolean, unBooleanGrep],
    [unUndefined, unUndefinedGrep],
    [unInfinity, unInfinityGrep],
])

export const transformationRules = _transformationRules.map((rule) => {
    if (ruleAlternativesOnNodejs.get(rule)) {
        return ruleAlternativesOnNodejs.get(rule)!
    }
    return rule
})

export {
    transformationRuleIds,
}
