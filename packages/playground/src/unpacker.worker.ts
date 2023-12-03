import { unpack } from '@wakaru/unpacker'

onmessage = (
    input: MessageEvent<string>,
) => {
    try {
        const result = unpack(input.data)
        postMessage(result)
    }
    catch (e) {
        // We print the error here because it will lose the stack trace after being sent to the main thread
        console.error(e)
        throw e
    }
}
