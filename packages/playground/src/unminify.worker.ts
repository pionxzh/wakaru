import { runTransformationRules } from '@wakaru/unminify'
import type { CodeModParams, TransformedModule } from './types'

onmessage = async (
    msg: MessageEvent<CodeModParams>,
) => {
    try {
        const { name, module, transformationRuleIds, moduleMeta, moduleMapping } = msg.data
        const fileInfo = { path: name, source: module.code }
        const params = { moduleMeta, moduleMapping }
        const { code } = await runTransformationRules(fileInfo, transformationRuleIds, params)
        const transformedDep: TransformedModule = { ...module, transformed: code }
        postMessage(transformedDep)
    }
    catch (e) {
        // We print the error here because it will lose the stack trace after being sent to the main thread
        console.error(e)
        throw e
    }
}
