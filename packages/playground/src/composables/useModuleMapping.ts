import { KEY_MODULE_MAPPING } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'
import type { ModuleMapping } from '@unminify-kit/unpacker'

export function useModuleMapping() {
    const [moduleMapping, setModuleMapping] = useLocalStorage<ModuleMapping>(KEY_MODULE_MAPPING, {})

    return {
        moduleMapping,
        setModuleMapping,
    }
}
