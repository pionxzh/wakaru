import CodemodWorker from '../codemod.worker?worker'
import type { TransformedModule } from '../types'
import type { ModuleMapping } from '@unminify-kit/unpacker'

export function useCodemod() {
    const transform = (name: string, module: TransformedModule, transformations: string[], moduleMapping: ModuleMapping) => {
        return new Promise<TransformedModule>((resolve, reject) => {
            const codemodWorker = new CodemodWorker()

            codemodWorker.onmessage = ({ data }: MessageEvent<TransformedModule>) => {
                codemodWorker.terminate()
                resolve(data)
            }

            codemodWorker.onerror = (error) => {
                codemodWorker.terminate()
                reject(error)
            }

            codemodWorker.postMessage({ name, module, transformations, moduleMapping })
        })
    }

    return {
        transform,
    }
}
