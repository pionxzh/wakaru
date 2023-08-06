import jscodeshift from 'jscodeshift'

// @ts-expect-error - no types
import getParser from 'jscodeshift/src/getParser'

import { getModulesFromBrowserify } from './extractors/browserify'
import { getModulesFromWebpack } from './extractors/webpack'

/**
 * Unpacks the given source code from supported bundlers.
 */
export function unpack(sourceCode: string) {
    const parser = getParser()
    const j = jscodeshift.withParser(parser)
    const root = j(sourceCode)

    const result
     = getModulesFromWebpack(j, root)
    || getModulesFromBrowserify(j, root)

    if (!result) {
        console.error('Failed to locate modules')
        return null
    }

    const { modules, moduleIdMapping } = result

    return {
        modules,
        moduleIdMapping,
    }
}
