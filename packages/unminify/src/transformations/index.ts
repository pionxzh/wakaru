// @ts-expect-error - no types
import cjs from '5to6-codemod/transforms/cjs.js'

// @ts-expect-error - no types
import exports from '5to6-codemod/transforms/exports.js'
import functionToArrow from './function-to-arrow'
import lebab from './lebab'
import moduleMapping from './module-mapping'
import prettier from './prettier'
import unBoolean from './un-boolean'
import unEsHelper from './un-es-helper'
import unES6Class from './un-es6-class'
import unExportRename from './un-export-rename'
import unFlipComparisons from './un-flip-operator'
import unIfStatement from './un-if-statement'
import unNumberLiteral from './un-number-literal'
import unSequenceExpression from './un-sequence-expression'
import unSwitchStatement from './un-switch-statement'
import unUseStrict from './un-use-strict'
import unVariableMerging from './un-variable-merging'
import unVoid0 from './un-void-0'
import type { Transform } from 'jscodeshift'

export const transformationMap: {
    [name: string]: Transform
} = {
    'module-mapping': moduleMapping,
    'un-sequence-expression1': unSequenceExpression,
    lebab,
    cjs,
    exports,
    'un-export-rename': unExportRename,
    'un-use-strict': unUseStrict,
    'un-es-helper': unEsHelper,
    'un-boolean': unBoolean,
    'un-void-0': unVoid0,
    'un-number-literal': unNumberLiteral,
    'un-flip-comparisons': unFlipComparisons,
    'un-variable-merging': unVariableMerging,
    'un-sequence-expression2': unSequenceExpression,
    'un-switch-statement': unSwitchStatement,
    'un-if-statement': unIfStatement,
    'un-sequence-expression3': unSequenceExpression,
    'un-es6-class': unES6Class,
    'function-to-arrow': functionToArrow,
    prettier,
}
