import { KEY_MODULE_META } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'
import type { ModuleMeta } from '@wakaru/ast-utils/types'

export const useModuleMeta = () => {
    const [moduleMeta, setModuleMeta] = useLocalStorage<ModuleMeta>(KEY_MODULE_META, {})
    return {
        moduleMeta,
        setModuleMeta,
    }
}
