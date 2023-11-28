import lebab from './lebab'
import moduleMapping from './module-mapping'
import moduleMappingGrep from './module-mapping.grep'
import prettier from './prettier'
import smartInline from './smart-inline'
import smartRename from './smart-rename'
import unArgumentSpread from './un-argument-spread'
import unAssignmentMerging from './un-assignment-merging'
import unAsyncAwait from './un-async-await'
import unBoolean from './un-boolean'
import unBooleanGrep from './un-boolean.grep'
import unBracketNotation from './un-bracket-notation'
import unBuiltinPrototype from './un-builtin-prototype'
import unConditionals from './un-conditionals'
import unCurlyBraces from './un-curly-braces'
import unEnum from './un-enum'
import unES6Class from './un-es6-class'
import unEsm from './un-esm'
import unEsModuleFlag from './un-esmodule-flag'
import unExportRename from './un-export-rename'
import unFlipComparisons from './un-flip-comparisons'
import unIife from './un-iife'
import unImportRename from './un-import-rename'
import unIndirectCall from './un-indirect-call'
import unInfinity from './un-infinity'
import unInfinityGrep from './un-infinity.grep'
import unJsx from './un-jsx'
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
import unUndefinedGrep from './un-undefined.grep'
import unUseStrict from './un-use-strict'
import unUseStrictGrep from './un-use-strict.grep'
import unVariableMerging from './un-variable-merging'
import unWhileLoop from './un-while-loop'
import type { TransformationRule } from '@wakaru/shared/rule'

export const transformationRules: TransformationRule[] = [
    // first stage - basically prettify the code
    prettier.withId('prettier'),
    moduleMapping,
    unCurlyBraces, // add curly braces so that other transformations can works easier, but generally this is not required
    unSequenceExpression, // curly braces can bring out return sequence expression, so it runs before this
    unVariableMerging,
    unAssignmentMerging,

    // second stage - prepare the code for unminify
    unRuntimeHelper, // eliminate helpers as early as possible
    unEsm, // relies on `un-runtime-helper` to eliminate helpers around `require` calls, relies on `un-assignment-merging` to split exports
    unEnum, // relies on `un-sequence-expression`

    // third stage - mostly one-to-one transformation
    lebab,
    unExportRename, // relies on `un-esm` to give us the export statements, and this can break some rules from `lebab`
    unUseStrict,
    unEsModuleFlag,
    unBoolean,
    unUndefined,
    unInfinity,
    unTypeof,
    unNumericLiteral,
    unTemplateLiteral,
    unBracketNotation,
    unReturn,
    unWhileLoop,
    unIndirectCall,
    unTypeConstructor,
    unBuiltinPrototype,
    unSequenceExpression,
    unFlipComparisons,

    // advanced syntax upgrade
    unIife,
    unImportRename, // should run after `un-esm` to cover all the import statements
    smartInline, // relies on `lebab`'s `let` to `const` and `un-sequence-expression`
    smartRename, // partially relies on `un-indirect-call` to work
    unOptionalChaining,
    unNullishCoalescing,
    unConditionals, // need to run after `un-optional-chaining` and `un-nullish-coalescing`
    unSequenceExpression, // split sequence expressions introduced by `un-conditionals`
    unParameters, // relies on `un-flip-comparisons` to work
    unArgumentSpread,
    unJsx,
    unES6Class,
    unAsyncAwait,

    // last stage - prettify the code again after we finish all the transformations
    prettier.withId('prettier-1'),
]

const astGrepRules: TransformationRule[] = [
    unUseStrictGrep,
    unUndefinedGrep,
    moduleMappingGrep,
    unInfinityGrep,
    unBooleanGrep,
]

// replace the transform function in transformationRules with the one from astGrepRules
export const transformationRulesForCLI: TransformationRule[] = transformationRules.map((rule) => {
    const astGrepRule = astGrepRules.find(r => r.name === rule.name)
    return astGrepRule ?? rule
})
