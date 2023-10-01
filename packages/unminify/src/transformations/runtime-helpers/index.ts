import wrap from '../../wrapAstTransformation'
import { transformAST as arrayLikeToArray } from './babel/arrayLikeToArray'
import { transformAST as arrayWithoutHoles } from './babel/arrayWithoutHoles'
import { transformAST as objectSpread } from './babel/objectSpread'
import { transformAST as slicedToArray } from './babel/slicedToArray'
import { transformAST as toConsumableArray } from './babel/toConsumableArray'
import type { ASTTransformation } from '../../wrapAstTransformation'

export const transformAST: ASTTransformation = (context, params) => {
    // babel helpers
    arrayLikeToArray(context, params)
    arrayWithoutHoles(context, params)
    objectSpread(context, params)
    toConsumableArray(context, params)
    slicedToArray(context, params)
}

export default wrap(transformAST)
