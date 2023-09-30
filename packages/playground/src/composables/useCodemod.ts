import CodemodWorker from '../codemod.worker?worker'
import type { ModuleMeta } from './useModuleMeta'
import type { TransformedModule } from '../types'
import type { ModuleMapping } from '@unminify-kit/ast-utils'

export function useCodemod() {
    const transform = (
        name: string,
        module: TransformedModule,
        transformations: string[],
        moduleMeta: ModuleMeta,
        moduleMapping: ModuleMapping,
    ) => {
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

            codemodWorker.postMessage({ name, module, transformations, moduleMeta, moduleMapping })
        })
    }

    return {
        transform,
    }
}
