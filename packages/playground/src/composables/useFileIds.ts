import { KEY_FILE_ID_LIST } from '../const'
import { useLocalStorage } from './shared/useLocalStorage'
import type { FileIdList } from '../types'

export function useFileIds() {
    const [fileIds, setFileIds] = useLocalStorage<FileIdList>(KEY_FILE_ID_LIST, [])
    return {
        fileIds,
        setFileIds,
    }
}
