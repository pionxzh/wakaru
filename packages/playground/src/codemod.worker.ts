import { runTransformations, transformationMap } from '@wakaru/unminify'
import type { TransformedModule } from './types'
import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'

onmessage = (
    msg: MessageEvent<{
        name: string
        module: TransformedModule
        transformations: string[]
        moduleMeta: ModuleMeta
        moduleMapping: ModuleMapping
    }>,
) => {
    const { name, module, moduleMeta, moduleMapping } = msg.data
    const fileInfo = { path: name, source: module.code }
    const transforms = msg.data.transformations?.map(t => transformationMap[t]) ?? Object.values(transformationMap)
    const { code } = runTransformations(fileInfo, transforms, { moduleMeta, moduleMapping })
    const transformedDep: TransformedModule = { ...module, transformed: code }
    postMessage(transformedDep)
}
