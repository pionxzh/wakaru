import { KEY_FILE_PREFIX } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'
import type { TransformedModule } from '../types'

export function useModule(id: number | string) {
    const emptyModule: TransformedModule = {
        id: 0,
        code: '',
        transformed: '',
        isEntry: true,
    }

    const [module, setModule] = useLocalStorage<TransformedModule>(`${KEY_FILE_PREFIX}${id}`, emptyModule)
    return {
        module,
        setModule,
    }
}
