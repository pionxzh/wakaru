import UnpackerWorker from '../unpacker.worker?worker'
import type { UnpackerResult } from '../types'

export function useUnpacker() {
    return (input: string) => {
        return new Promise<UnpackerResult>((resolve, reject) => {
            const worker = new UnpackerWorker()

            worker.onmessage = ({ data }: MessageEvent<UnpackerResult>) => {
                worker.terminate()
                resolve(data)
            }

            worker.onerror = (error) => {
                worker.terminate()
                reject(error)
            }

            worker.postMessage(input)
        })
    }
}
