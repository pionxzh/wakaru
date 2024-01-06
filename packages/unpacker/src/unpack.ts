import { nonNullable } from '@wakaru/shared/array'
import { jscodeshiftWithParser as j, printSourceWithErrorLoc } from '@wakaru/shared/jscodeshift'
import { getModulesFromBrowserify } from './extractors/browserify'
import { getModulesFromWebpack } from './extractors/webpack'
import { Module } from './Module'
import { postScanRuntime, scanExports, scanImports, scanRuntime } from './module-scan'
import type { ModuleMapping } from '@wakaru/ast-utils/types'
import type { Collection } from 'jscodeshift'

type ModuleWithRoot = Module & { root: Collection }

/**
 * Unpacks the given source code from supported bundlers.
 */
export function unpack(sourceCode: string): {
    modules: Module[]
    moduleIdMapping: ModuleMapping
} {
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

    // module as key, root as value
    const modulesArray = [...modules]
    const modulesWithRoot = modulesArray.map<ModuleWithRoot | null>((module) => {
        try {
            const root = j(module.code)
            return { ...module, root }
        }
        catch (err: any) {
            console.error(`\nFailed to parse module ${module.id} with jscodeshift`, err)
            printSourceWithErrorLoc(err, module.code)

            return null
        }
    }).filter(nonNullable)

    modulesWithRoot.forEach((module) => {
        scanImports(j, module)
        scanExports(j, module)
    })

    modulesWithRoot.forEach((module) => {
        scanRuntime(j, module)
    })

    postScanRuntime(j, modulesWithRoot)

    return {
        modules: modulesArray,
        moduleIdMapping,
    }
}
