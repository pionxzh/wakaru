import { transformationMap } from '@unminify-kit/unminify'
import { KEY_TRANSFORMATIONS } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'

export const useTransformationRules = () => {
    const allRules = Object.keys(transformationMap)
    const [enabledRules, setEnabledRules] = useLocalStorage(KEY_TRANSFORMATIONS, allRules)

    return {
        allRules,
        enabledRules,
        setEnabledRules,
    }
}
