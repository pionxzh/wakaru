import lebab from './lebab'
import moduleMapping from './module-mapping'
import prettier from './prettier'
import smartInline from './smart-inline'
import smartRename from './smart-rename'
import unAssignmentMerging from './un-assignment-merging'
import unAsyncAwait from './un-async-await'
import unBoolean from './un-boolean'
import unBracketNotation from './un-bracket-notation'
import unBuiltinPrototype from './un-builtin-prototype'
import unConditionals from './un-conditionals'
import unCurlyBraces from './un-curly-braces'
import unEnum from './un-enum'
import unES6Class from './un-es6-class'
import unEsm, { Schema as UnEsmSchema } from './un-esm'
import unEsModuleFlag from './un-esmodule-flag'
import unExportRename from './un-export-rename'
import unFlipComparisons from './un-flip-comparisons'
import unIife from './un-iife'
import unIndirectCall from './un-indirect-call'
import unInfinity from './un-infinity'
import unJsx, { Schema as UnJsxSchema } from './un-jsx'
import unNullishCoalescing from './un-nullish-coalescing'
import unNumericLiteral from './un-numeric-literal'
import unOptionalChaining from './un-optional-chaining'
import unParameters from './un-parameters'
import unReturn from './un-return'
import unRuntimeHelper from './un-runtime-helper'
import unSequenceExpression from './un-sequence-expression'
import unTemplateLiteral from './un-template-literal'
import unTypeConstructor from './un-type-constructor'
import unTypeof from './un-typeof'
import unUndefined from './un-undefined'
import unUseStrict from './un-use-strict'
import unVariableMerging from './un-variable-merging'
import unWhileLoop from './un-while-loop'
import type { Transform } from 'jscodeshift'
import type { ZodSchema } from 'zod'

export const transformationMap: {
    [name: string]: Transform
} = {
    // first stage - basically prettify the code
    prettier,
    'module-mapping': moduleMapping,
    'un-curly-braces': unCurlyBraces, // add curly braces so that other transformations can works easier, but generally this is not required
    'un-sequence-expression1': unSequenceExpression, // curly braces can bring out return sequence expression, so it runs before this
    'un-variable-merging': unVariableMerging,
    'un-assignment-merging': unAssignmentMerging,

    // second stage - prepare the code for unminify
    'un-runtime-helper': unRuntimeHelper, // eliminate helpers as early as possible
    'un-esm': unEsm, // relies on `un-runtime-helper` to eliminate helpers around `require` calls, relies on `un-assignment-merging` to split exports
    'un-enum': unEnum, // relies on `un-sequence-expression`

    // third stage - mostly one-to-one transformation
    lebab,
    'un-export-rename': unExportRename, // relies on `un-esm` to give us the export statements, and this can break some rules from `lebab`
    'un-use-strict': unUseStrict,
    'un-esmodule-flag': unEsModuleFlag,
    'un-boolean': unBoolean,
    'un-undefined': unUndefined,
    'un-infinity': unInfinity,
    'un-typeof': unTypeof,
    'un-numeric-literal': unNumericLiteral,
    'un-template-literal': unTemplateLiteral,
    'un-bracket-notation': unBracketNotation,
    'un-return': unReturn,
    'un-while-loop': unWhileLoop,
    'un-indirect-call': unIndirectCall,
    'un-type-constructor': unTypeConstructor,
    'un-builtin-prototype': unBuiltinPrototype,
    'un-sequence-expression2': unSequenceExpression,
    'un-flip-comparisons': unFlipComparisons,

    // advanced syntax upgrade
    'smart-inline': smartInline, // relies on `lebab`'s `let` to `const` and `un-sequence-expression`
    'smart-rename': smartRename, // partially relies on `un-indirect-call` to work
    'un-optional-chaining': unOptionalChaining,
    'un-nullish-coalescing': unNullishCoalescing,
    'un-conditionals': unConditionals, // need to run after `un-optional-chaining` and `un-nullish-coalescing`
    'un-sequence-expression3': unSequenceExpression, // split sequence expressions introduced by `un-conditionals`
    'un-parameters': unParameters, // relies on `un-flip-comparisons` to work
    'un-jsx': unJsx,
    'un-iife': unIife,
    'un-es6-class': unES6Class,
    'un-async-await': unAsyncAwait,

    // last stage - prettify the code again after we finish all the transformations
    'prettier-last': prettier,
}

export interface TransformationRule {
    id: string
    /**
     * Name of the transformation rule
     */
    name: string
    /**
     * The transform function (impl)
     */
    transform: Transform
    /**
     * The zod schema for the options
     */
    schema?: ZodSchema
}

const _transformationRules = [
    // first stage - basically prettify the code
    { name: 'prettier', transform: prettier },
    { name: 'module-mapping', transform: moduleMapping },
    { name: 'un-curly-braces', transform: unCurlyBraces }, // add curly braces so that other transformations can works easier, but generally this is not required
    { name: 'un-sequence-expression', transform: unSequenceExpression }, // curly braces can bring out return sequence expression, so it runs before this
    { name: 'un-variable-merging', transform: unVariableMerging },
    { name: 'un-assignment-merging', transform: unAssignmentMerging },

    // second stage - prepare the code for unminify
    { name: 'un-runtime-helper', transform: unRuntimeHelper }, // eliminate helpers as early as possible
    { name: 'un-esm', transform: unEsm, schema: UnEsmSchema }, // relies on `un-runtime-helper` to eliminate helpers around `require` calls, relies on `un-assignment-merging` to split exports
    { name: 'un-enum', transform: unEnum }, // relies on `un-sequence-expression`

    // third stage - mostly one-to-one transformation
    { name: 'lebab', transform: lebab },
    { name: 'un-export-rename', transform: unExportRename }, // relies on `un-esm` to give us the export statements, and this can break some rules from `lebab`
    { name: 'un-use-strict', transform: unUseStrict },
    { name: 'un-esmodule-flag', transform: unEsModuleFlag },
    { name: 'un-boolean', transform: unBoolean },
    { name: 'un-undefined', transform: unUndefined },
    { name: 'un-infinity', transform: unInfinity },
    { name: 'un-typeof', transform: unTypeof },
    { name: 'un-numeric-literal', transform: unNumericLiteral },
    { name: 'un-template-literal', transform: unTemplateLiteral },
    { name: 'un-bracket-notation', transform: unBracketNotation },
    { name: 'un-return', transform: unReturn },
    { name: 'un-while-loop', transform: unWhileLoop },
    { name: 'un-indirect-call', transform: unIndirectCall },
    { name: 'un-type-constructor', transform: unTypeConstructor },
    { name: 'un-builtin-prototype', transform: unBuiltinPrototype },
    { name: 'un-sequence-expression', transform: unSequenceExpression },
    { name: 'un-flip-comparisons', transform: unFlipComparisons },

    // advanced syntax upgrade
    { name: 'smart-inline', transform: smartInline }, // relies on `lebab`'s `let` to `const` and `un-sequence-expression`
    { name: 'smart-rename', transform: smartRename }, // partially relies on `un-indirect-call` to work
    { name: 'un-optional-chaining', transform: unOptionalChaining },
    { name: 'un-nullish-coalescing', transform: unNullishCoalescing },
    { name: 'un-conditionals', transform: unConditionals }, // need to run after `un-optional-chaining` and `un-nullish-coalescing`
    { name: 'un-sequence-expression', transform: unSequenceExpression }, // split sequence expressions introduced by `un-conditionals`
    { name: 'un-parameters', transform: unParameters }, // relies on `un-flip-comparisons` to work
    { name: 'un-jsx', transform: unJsx, schema: UnJsxSchema },
    { name: 'un-iife', transform: unIife },
    { name: 'un-es6-class', transform: unES6Class },
    { name: 'un-async-await', transform: unAsyncAwait },

    // last stage - prettify the code again after we finish all the transformations
    { name: 'prettier', transform: prettier },
]

const occurrenceMap = new Map<string, number>()
export const transformationRules: TransformationRule[] = _transformationRules.map((rule) => {
    const occurrence = occurrenceMap.get(rule.name) ?? 0
    occurrenceMap.set(rule.name, occurrence + 1)
    const id = occurrence === 0 ? rule.name : `${rule.name}-${occurrence}`
    // Set the name of the function for better debugging
    Object.defineProperty(rule.transform, 'name', { value: id })
    return { ...rule, id }
})
