import { transformationMap } from '@wakaru/unminify'
import { ref } from 'vue'
import { KEY_DISABLED_RULES } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'

export const useTransformationRules = () => {
    const allRules = ref(Object.keys(transformationMap))
    const [disabledRules, setDisabledRules] = useLocalStorage<string[]>(KEY_DISABLED_RULES, [])

    return {
        allRules,
        disabledRules,
        setDisabledRules,
    }
}
