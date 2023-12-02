import { runTransformationIds } from '@wakaru/unminify'
import type { TransformedModule } from './types'
import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'

onmessage = (
    msg: MessageEvent<{
        name: string
        module: TransformedModule
        transformationRuleIds: string[]
        moduleMeta: ModuleMeta
        moduleMapping: ModuleMapping
    }>,
) => {
    const { name, module, transformationRuleIds, moduleMeta, moduleMapping } = msg.data
    const fileInfo = { path: name, source: module.code }
    const { code } = runTransformationIds(fileInfo, transformationRuleIds, { moduleMeta, moduleMapping })
    const transformedDep: TransformedModule = { ...module, transformed: code }
    postMessage(transformedDep)
}
