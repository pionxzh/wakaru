import lebab from './lebab'
import moduleMapping from './module-mapping'
import prettier from './prettier'
import unAsyncAwait from './un-async-await'
import unBoolean from './un-boolean'
import unBracketNotation from './un-bracket-notation'
import unBuiltinPrototype from './un-builtin-prototype'
import unCurlyBraces from './un-curly-braces'
import unES6Class from './un-es6-class'
import unEsm from './un-esm'
import unEsModuleFlag from './un-esmodule-flag'
import unExportRename from './un-export-rename'
import unFlipComparisons from './un-flip-operator'
import unIfStatement from './un-if-statement'
import unIife from './un-iife'
import unInfinity from './un-infinity'
import unNumericLiteral from './un-numeric-literal'
import unSequenceExpression from './un-sequence-expression'
import unSwitchStatement from './un-switch-statement'
import unTemplateLiteral from './un-template-literal'
import unTypeConstructor from './un-type-constructor'
import unUndefined from './un-undefined'
import unUseStrict from './un-use-strict'
import unVariableMerging from './un-variable-merging'
import unWhileLoop from './un-while-loop'
import type { Transform } from 'jscodeshift'

export const transformationMap: {
    [name: string]: Transform
} = {
    prettier,
    'module-mapping': moduleMapping,
    'un-sequence-expression1': unSequenceExpression,
    lebab,
    'un-esm': unEsm,
    'un-export-rename': unExportRename,
    'un-use-strict': unUseStrict,
    'un-esmodule-flag': unEsModuleFlag,
    'un-boolean': unBoolean,
    'un-undefined': unUndefined,
    'un-infinity': unInfinity,
    'un-numeric-literal': unNumericLiteral,
    'un-template-literal': unTemplateLiteral,
    'un-curly-braces': unCurlyBraces,
    'un-while-loop': unWhileLoop,
    'un-bracket-notation': unBracketNotation,
    'un-flip-comparisons': unFlipComparisons,
    'un-variable-merging': unVariableMerging,
    'un-type-constructor': unTypeConstructor,
    'un-builtin-prototype': unBuiltinPrototype,
    'un-sequence-expression2': unSequenceExpression,
    'un-switch-statement': unSwitchStatement,
    'un-if-statement': unIfStatement,
    'un-sequence-expression3': unSequenceExpression,
    'un-iife': unIife,
    'un-es6-class': unES6Class,
    'un-async-await': unAsyncAwait,
    'prettier-last': prettier,
}
