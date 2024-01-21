import { transformationRules } from '@wakaru/unminify'
import { atomWithStorage } from 'jotai/utils'
import { atom } from 'jotai/vanilla'
import { KEY_DISABLED_RULES, KEY_RULE_ORDER } from '../const'

export const prettifyRules = [
    'un-sequence-expression1',
    'un-variable-merging',
    'prettier',
]

export const allRulesAtom = atom(() => transformationRules)

export const ruleOrderAtom = atomWithStorage<string[]>(KEY_RULE_ORDER, transformationRules.map(rule => rule.id))

export const orderedRulesAtom = atom((get) => {
    const ruleOrder = get(ruleOrderAtom)
    const allRules = get(allRulesAtom)
    return allRules.toSorted((a, b) => {
        const aIndex = ruleOrder.indexOf(a.id)
        const bIndex = ruleOrder.indexOf(b.id)
        if (aIndex === -1 && bIndex === -1) return 0
        if (aIndex === -1) return 1
        if (bIndex === -1) return -1
        return aIndex - bIndex
    })
})

export const disabledRuleIdsAtom = atomWithStorage<string[]>(KEY_DISABLED_RULES, [])

export const disabledRulesAtom = atom((get) => {
    const disabledRuleIds = get(disabledRuleIdsAtom)
    const allRules = get(allRulesAtom)
    return allRules.filter(rule => disabledRuleIds.includes(rule.id))
})

export const enabledRulesAtom = atom((get) => {
    const orderedRules = get(orderedRulesAtom)
    const disabledRuleIds = get(disabledRuleIdsAtom)
    return orderedRules.filter(rule => !disabledRuleIds.includes(rule.id))
})

export const enabledRuleIdsAtom = atom((get) => {
    const enabledRules = get(enabledRulesAtom)
    return enabledRules.map(rule => rule.id)
})

export const toggleRuleAtom = atom(null, (get, set, ruleId: string) => {
    const disabledRuleIds = get(disabledRuleIdsAtom)
    if (disabledRuleIds.includes(ruleId)) {
        set(disabledRuleIdsAtom, disabledRuleIds.filter(id => id !== ruleId))
    }
    else {
        set(disabledRuleIdsAtom, [...disabledRuleIds, ruleId])
    }
})

export const resetRulesAtom = atom(null, (_get, set) => {
    set(disabledRuleIdsAtom, [])
    set(ruleOrderAtom, transformationRules.map(rule => rule.id))
})
