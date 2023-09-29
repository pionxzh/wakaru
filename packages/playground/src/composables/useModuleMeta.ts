import { KEY_MODULE_META } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'
import type { Module } from '@unminify-kit/unpacker'

export interface ModuleMeta {
    [moduleId: string]: Pick<Module, 'import' | 'export' | 'tags'>
}

export const useModuleMeta = () => {
    const [moduleMeta, setModuleMeta] = useLocalStorage<ModuleMeta>(KEY_MODULE_META, {})
    return {
        moduleMeta,
        setModuleMeta,
    }
}
