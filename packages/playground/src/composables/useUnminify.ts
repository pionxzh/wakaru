import CodemodWorker from '../unminify.worker?worker'
import type { CodeModParams, TransformedModule } from '../types'

export function useUnminify() {
    const transform = (param: CodeModParams) => {
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

            codemodWorker.postMessage(param)
        })
    }

    return {
        transform,
    }
}
