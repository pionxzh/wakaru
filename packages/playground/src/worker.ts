import { EsThread, EsThreadPool } from 'threads-es/controller'
import UnminifyWorker from './unminify.worker?worker'
import UnpackerWorker from './unpacker.worker?worker'
import type { CodeModParams } from './types'
import type { UnminifyApiType } from './unminify.worker'
import type { UnpackerApiType } from './unpacker.worker'

const unminifyPool = await EsThreadPool.Spawn(() => EsThread.Spawn<UnminifyApiType>(new UnminifyWorker()), { size: 4 })
const unpackerPool = await EsThreadPool.Spawn(() => EsThread.Spawn<UnpackerApiType>(new UnpackerWorker()), { size: 1 })

export async function unminify(param: CodeModParams) {
    return unminifyPool.queue(thread => thread.methods.execute(param))
}

export async function unpack(input: string) {
    return unpackerPool.queue(thread => thread.methods.execute(input))
}
