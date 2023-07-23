import type { LebabRule } from 'lebab'
import { transform } from 'lebab'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'

const allLebabRules: LebabRule[] = [
    // 'class',
    'template',
    'arrow',
    'arrow-return',
    'let',
    'default-param',
    // 'destruct-param',
    'arg-spread',
    'arg-rest',
    'obj-method',
    'obj-shorthand',
    'no-strict',
    'commonjs',
    'exponent',
    'multi-var',
    'for-of',
    'for-each',
    'includes',
]

function transformLebab(input: string) {
    const { code, warnings } = transform(input, allLebabRules)
    return { code, warnings }
}

/**
 * @url https://github.com/lebab/lebab
 */
export const transformAST: ASTTransformation = (context) => {
    const code = context.root.toSource()
    return transformLebab(code).code
}

export default wrap(transformAST)
