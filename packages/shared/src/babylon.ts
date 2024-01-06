/**
 * Forked from https://github.com/facebook/jscodeshift/blob/51da1a5c4ba3707adb84416663634d4fc3141cbb/parser/babylon.js#L12
 */

import * as babylon from '@babel/parser'
import type { ParserOptions } from '@babel/parser'

const defaultOptions: ParserOptions = {
    sourceType: 'module',
    allowImportExportEverywhere: true,
    allowReturnOutsideFunction: true,
    startLine: 1,
    tokens: true,
    plugins: [
        // ['flow', { all: true }],  // We don't use flow
        // 'flowComments',           // We don't use flow
        'jsx',

        // 'asyncGenerators',        // enabled by default
        // 'bigInt',                 // enabled by default
        // 'classProperties',        // enabled by default
        // 'classPrivateProperties', // enabled by default
        // 'classPrivateMethods',    // enabled by default
        ['decorators', { decoratorsBeforeExport: false }],
        'doExpressions',
        // 'dynamicImport',          // enabled by default
        'exportDefaultFrom',
        // 'exportNamespaceFrom',    // enabled by default
        'functionBind',
        'functionSent',
        'importMeta',
        // 'logicalAssignment',      // enabled by default
        // 'nullishCoalescingOperator', // enabled by default
        // 'numericSeparator',       // enabled by default
        // 'objectRestSpread',       // enabled by default
        // 'optionalCatchBinding',   // enabled by default
        // 'optionalChaining',       // enabled by default
        ['pipelineOperator', { proposal: 'minimal' }],
        'throwExpressions',
    ],
}

/**
 * Wrapper to set default options
 */
export default function (options = defaultOptions) {
    return {
        parse(code: string) {
            return babylon.parse(code, options)
        },
    }
}
