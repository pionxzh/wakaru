import { createStringTransformationRule } from '@wakaru/shared/rule'
import { transform } from 'lebab'
import type { StringTransformation } from '@wakaru/shared/rule'
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

export const transformASTWithRules = (rules: LebabRule[]): StringTransformation => (code) => {
    return transformLebab(code, rules).code
}

export default createStringTransformationRule({
    name: 'lebab',
    transform: transformASTWithRules(allLebabRules),
})
