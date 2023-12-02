import { KEY_MODULE_MAPPING } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'
import type { ModuleMapping } from '@wakaru/ast-utils/types'

export function useModuleMapping() {
    const [moduleMapping, setModuleMapping] = useLocalStorage<ModuleMapping>(KEY_MODULE_MAPPING, {})

    return {
        moduleMapping,
        setModuleMapping,
    }
}
