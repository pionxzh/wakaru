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
import unEsm from './un-esm'
import unEsModuleFlag from './un-esmodule-flag'
import unExportRename from './un-export-rename'
import unFlipComparisons from './un-flip-comparisons'
import unIife from './un-iife'
import unIndirectCall from './un-indirect-call'
import unInfinity from './un-infinity'
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
import unUseStrict from './un-use-strict'
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
    smartInline, // relies on `lebab`'s `let` to `const` and `un-sequence-expression`
    smartRename, // partially relies on `un-indirect-call` to work
    unOptionalChaining,
    unNullishCoalescing,
    unConditionals, // need to run after `un-optional-chaining` and `un-nullish-coalescing`
    unSequenceExpression, // split sequence expressions introduced by `un-conditionals`
    unParameters, // relies on `un-flip-comparisons` to work
    unJsx,
    unIife,
    unES6Class,
    unAsyncAwait,

    // last stage - prettify the code again after we finish all the transformations
    prettier.withId('prettier-1'),
]
