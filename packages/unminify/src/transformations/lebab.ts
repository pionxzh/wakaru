import { transform } from 'lebab'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { LebabRule } from 'lebab'

const allLebabRules: LebabRule[] = [
    // 'class',
    'template', // un-template-literal, but lebab support a different form of template literal (+ instead of .concat)
    'arrow',
    'arrow-return',
    'let',
    'default-param',
    // 'destruct-param',
    'arg-spread',
    'arg-rest',
    'obj-method',
    'obj-shorthand',
    // 'no-strict', // un-use-strict
    // 'commonjs',  // un-esm
    'exponent',
    // 'multi-var', // un-variable-merging
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
