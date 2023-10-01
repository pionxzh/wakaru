import { getTopLevelStatements } from '@unminify-kit/ast-utils'
import { mergeComments } from '../utils/comments'
import wrap from '../wrapAstTransformation'
import { transformAST as babelHelpers } from './runtime-helpers'
import type { SharedParams } from '../utils/types'
import type { ASTTransformation, Context } from '../wrapAstTransformation'
import type { FunctionDeclaration } from 'jscodeshift'

const addAnnotationOnHelper = (context: Context, params: SharedParams) => {
    const { moduleMapping, moduleMeta } = params
    if (!moduleMapping || !moduleMeta) return

    const { root, j, filename } = context
    const moduleId = Object.entries(moduleMapping).find(([_, path]) => path === filename)?.[0]
    if (moduleId === undefined) return

    const modMeta = moduleMeta[moduleId]
    if (!modMeta) return

    const statements = getTopLevelStatements(root)
    const functions = statements.filter((s): s is FunctionDeclaration => j.FunctionDeclaration.check(s))

    functions.forEach((fn) => {
        if (!j.Identifier.check(fn.id)) return
        const functionName = fn.id.name

        const tags = modMeta.tags[functionName]
        if (!tags || tags.length === 0) return

        const commentContent = tags.map(tag => ` * ${tag}`).join('\n')
        const comment = j.commentBlock(`*\n${commentContent}\n `, true, false)
        mergeComments(fn, [comment])
    })
}

/**
 * Replace runtime helper with the actual original code.
 */
export const transformAST: ASTTransformation<SharedParams> = (context, params) => {
    addAnnotationOnHelper(context, params)

    babelHelpers(context, params)
}

export default wrap(transformAST)
