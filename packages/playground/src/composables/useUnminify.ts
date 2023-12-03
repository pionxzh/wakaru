import UnminifyWorker from '../unminify.worker?worker'
import type { CodeModParams, TransformedModule } from '../types'

export function useUnminify() {
    return (param: CodeModParams) => {
        return new Promise<TransformedModule>((resolve, reject) => {
            const worker = new UnminifyWorker()

            worker.onmessage = ({ data }: MessageEvent<TransformedModule>) => {
                worker.terminate()
                resolve(data)
            }

            worker.onerror = (error) => {
                worker.terminate()
                reject(error)
            }

            worker.postMessage(param)
        })
    }
}
