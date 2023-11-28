import { wrapAstTransformation } from '@wakaru/ast-utils'
import { transform } from 'lebab'
import type { ASTTransformation } from '@wakaru/ast-utils'
import type { LebabRule } from 'lebab'

/**
 * @url https://github.com/lebab/lebab
 */
const allLebabRules: LebabRule[] = [
    'class',
    'template', // un-template-literal, but lebab support a different form of template literal (+ instead of .concat)
    'arrow',
    'arrow-return',
    'let',
    // 'default-param',
    // 'destruct-param',
    // 'arg-spread',
    // 'arg-rest',
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

function transformLebab(input: string, rules: LebabRule[]) {
    const { code, warnings } = transform(input, rules)
    return { code, warnings }
}

export const transformASTWithRules = (rules: LebabRule[]): ASTTransformation => (context) => {
    const code = context.root.toSource()
    return transformLebab(code, rules).code
}

export default wrapAstTransformation(transformASTWithRules(allLebabRules))
