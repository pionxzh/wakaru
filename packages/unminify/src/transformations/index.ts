import lebab from './lebab'
import moduleMapping from './module-mapping'
import prettier from './prettier'
import unAsyncAwait from './un-async-await'
import unBoolean from './un-boolean'
import unBracketNotation from './un-bracket-notation'
import unEsHelper from './un-es-helper'
import unES6Class from './un-es6-class'
import unEsm from './un-esm'
import unExportRename from './un-export-rename'
import unFlipComparisons from './un-flip-operator'
import unIfStatement from './un-if-statement'
import unInfinity from './un-infinity'
import unNumberLiteral from './un-number-literal'
import unSequenceExpression from './un-sequence-expression'
import unSwitchStatement from './un-switch-statement'
import unTemplateLiteral from './un-template-literal'
import unUseStrict from './un-use-strict'
import unVariableMerging from './un-variable-merging'
import unVoid0 from './un-void-0'
import unWhile from './un-while'
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
    'un-es-helper': unEsHelper,
    'un-boolean': unBoolean,
    'un-void-0': unVoid0,
    'un-infinity': unInfinity,
    'un-number-literal': unNumberLiteral,
    'un-template-literal': unTemplateLiteral,
    'un-while': unWhile,
    'un-bracket-notation': unBracketNotation,
    'un-flip-comparisons': unFlipComparisons,
    'un-variable-merging': unVariableMerging,
    'un-sequence-expression2': unSequenceExpression,
    'un-switch-statement': unSwitchStatement,
    'un-if-statement': unIfStatement,
    'un-sequence-expression3': unSequenceExpression,
    'un-es6-class': unES6Class,
    'un-async-await': unAsyncAwait,
    'prettier-last': prettier,
}
