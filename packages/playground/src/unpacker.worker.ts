import { unpack } from '@wakaru/unpacker'
import { exposeApi } from 'threads-es/worker'

const UnpackerApi = {
    execute: async (input: string) => {
        try {
            return unpack(input)
        }
        catch (e) {
            // We print the error here because it will lose the stack trace after being sent to the main thread
            console.error(e)
            throw e
        }
    },
}

export type UnpackerApiType = typeof UnpackerApi

exposeApi(UnpackerApi)
