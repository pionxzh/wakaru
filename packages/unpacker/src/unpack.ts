import jscodeshift from 'jscodeshift'

// @ts-expect-error - no types
import getParser from 'jscodeshift/src/getParser'
import { getModulesFromBrowserify } from './extractors/browserify'
import { getModulesFromWebpack } from './extractors/webpack'
import { Module } from './Module'
import type { ModuleMapping } from './ModuleMapping'

/**
 * Unpacks the given source code from supported bundlers.
 */

export function unpack(sourceCode: string): {
    modules: Module[]
    moduleIdMapping: ModuleMapping
} {
    const parser = getParser()
    const j = jscodeshift.withParser(parser)
    const root = j(sourceCode)

    const result = getModulesFromWebpack(j, root)
        || getModulesFromBrowserify(j, root)

    if (!result) {
        // Fallback to a single module
        const module = new Module(0, j, root, true)
        return {
            modules: [module],
            moduleIdMapping: {},
        }
    }

    const { modules, moduleIdMapping } = result

    return {
        modules: [...modules],
        moduleIdMapping,
    }
}
