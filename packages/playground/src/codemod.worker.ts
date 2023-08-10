import { runTransformations, transformationMap } from '@unminify-kit/unminify'
import type { TransformedModule } from './types'
import type { ModuleMapping } from '@unminify-kit/unpacker'

onmessage = (
    msg: MessageEvent<{
        name: string
        module: TransformedModule
        transformations: string[]
        moduleMapping: ModuleMapping
    }>,
) => {
    const { name, module, moduleMapping } = msg.data
    const fileInfo = { path: name, source: module.code }
    const transforms = msg.data.transformations?.map(t => transformationMap[t]) ?? Object.values(transformationMap)
    const { code } = runTransformations(fileInfo, transforms, { moduleMapping })
    const transformedDep: TransformedModule = { ...module, transformed: code }
    postMessage(transformedDep)
}
