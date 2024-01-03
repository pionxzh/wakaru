import { mergeComments } from '@wakaru/ast-utils/comments'
import { getTopLevelStatements } from '@wakaru/ast-utils/program'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import { transformAST as babelHelpers } from './runtime-helpers'
import type { ASTTransformation, Context, SharedParams } from '@wakaru/shared/rule'
import type { FunctionDeclaration } from 'jscodeshift'

/**
 * Add annotation on runtime helper.
 */
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

        /**
         * {helperName}
         * {tag1}
         * {tag2}
         */
        const commentContent = tags.map(tag => ` * ${tag}`).join('\n')
        const comment = j.commentBlock(`*\n${commentContent}\n `, true, false)
        mergeComments(fn, [comment])
    })
}

/**
 * Replace runtime helper with the actual original code.
 */
export const transformAST: ASTTransformation = (context, params) => {
    addAnnotationOnHelper(context, params)

    babelHelpers(context, params)
}

export default createJSCodeshiftTransformationRule({
    name: 'un-runtime-helper',
    transform: transformAST,
})
