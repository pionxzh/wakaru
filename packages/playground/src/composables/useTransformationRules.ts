import { transformationMap } from '@wakaru/unminify'
import { KEY_DISABLED_RULES } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'

export const useTransformationRules = () => {
    const allRules = Object.keys(transformationMap)
    const [disabledRules, setDisabledRules] = useLocalStorage<string[]>(KEY_DISABLED_RULES, [])

    return {
        allRules,
        disabledRules,
        setDisabledRules,
    }
}
