import { transformationRules } from '@wakaru/unminify'
import { computed, ref } from 'vue'
import { KEY_DISABLED_RULES } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'

const dedupe = <T>(arr: T[]) => [...new Set(arr)]

export const useTransformationRules = () => {
    const allRules = ref(transformationRules)
    const [disabledRuleIds, setDisabledRuleIds] = useLocalStorage<string[]>(KEY_DISABLED_RULES, [])
    const enabledRules = computed(() => allRules.value.filter(rule => !disabledRuleIds.value.includes(rule.id)))

    const toggleRule = (ruleId: string) => {
        const index = disabledRuleIds.value.indexOf(ruleId)
        if (index === -1) {
            setDisabledRuleIds(dedupe([...disabledRuleIds.value, ruleId]))
        }
        else {
            setDisabledRuleIds([
                ...disabledRuleIds.value.slice(0, index),
                ...disabledRuleIds.value.slice(index + 1),
            ])
        }
    }

    return {
        allRules,
        enabledRules,
        disabledRuleIds,
        setDisabledRuleIds,
        toggleRule,
    }
}
