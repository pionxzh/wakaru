import { wrapAstTransformation } from '@wakaru/ast-utils'
import { findHelperLocals } from '../../../utils/import'
import { handleSpreadHelper } from './_spread'
import type { SharedParams } from '../../../utils/types'
import type { ASTTransformation } from '@wakaru/ast-utils'

/**
 * Restore object spread syntax from `@babel/runtime/helpers/extends` helper.
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-object-rest-spread
 * @see https://github.com/babel/babel/blob/b5d6c3c820af3c049b476df6e885fef33fa953f1/packages/babel-helpers/src/helpers.ts#L164-L180
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    const moduleName = '@babel/runtime/helpers/extends'
    const moduleEsmName = '@babel/runtime/helpers/esm/extends'

    const helperLocals = findHelperLocals(context, params, moduleName, moduleEsmName)
    handleSpreadHelper(context, helperLocals)
}

export default wrapAstTransformation(transformAST)
