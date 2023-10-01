import wrap from '../../wrapAstTransformation'
import { transformAST as arrayLikeToArray } from './arrayLikeToArray'
import { transformAST as arrayWithoutHoles } from './arrayWithoutHoles'
import { transformAST as objectSpread } from './objectSpread'
import { transformAST as slicedToArray } from './slicedToArray'
import { transformAST as toConsumableArray } from './toConsumableArray'
import type { ASTTransformation } from '../../wrapAstTransformation'

export const transformAST: ASTTransformation = (context, params) => {
    arrayLikeToArray(context, params)
    arrayWithoutHoles(context, params)
    objectSpread(context, params)
    toConsumableArray(context, params)
    slicedToArray(context, params)
}

export default wrap(transformAST)
