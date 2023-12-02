import jscodeshift from 'jscodeshift'

import { getModulesFromBrowserify } from './extractors/browserify'
import { getModulesFromWebpack } from './extractors/webpack'
import { Module } from './Module'
import { postScanRuntime, scanExports, scanImports, scanRuntime } from './module-scan'
import type { ModuleMapping } from '@wakaru/ast-utils/types'

/**
 * Unpacks the given source code from supported bundlers.
 */
export function unpack(sourceCode: string): {
    modules: Module[]
    moduleIdMapping: ModuleMapping
} {
    const j = jscodeshift.withParser('babylon')
    const root = j(sourceCode)

    const result = getModulesFromWebpack(j, root)
        || getModulesFromBrowserify(j, root)
        // Fallback to a single module
        || {
            modules: new Set([new Module(0, root.find(j.Program), true)]),
            moduleIdMapping: {
                0: 'entry.js',
            },
        }

    const { modules, moduleIdMapping } = result

    modules.forEach((module) => {
        scanImports(j, module)
        scanExports(j, module)
    })

    modules.forEach((module) => {
        scanRuntime(j, module)
    })

    const modulesArray = [...modules]
    postScanRuntime(j, modulesArray)

    return {
        modules: modulesArray,
        moduleIdMapping,
    }
}
