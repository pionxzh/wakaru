import { getTopLevelStatements } from '@unminify-kit/ast-utils'
import type { Module } from '../Module'
import type { ArrowFunctionExpression, FunctionDeclaration, FunctionExpression, JSCodeshift } from 'jscodeshift'

const moduleMatchers: Record<string, Array<string | RegExp | Array<string | RegExp>>> = {
    '@babel/runtime/helpers/classCallCheck': [
        'throw new TypeError("Cannot call a class as a function")',
    ],
    '@babel/runtime/helpers/createForOfIteratorHelperLoose': [
        'throw new TypeError("Invalid attempt to iterate non-iterable instance.\\nIn order to be iterable, non-array objects must have a [Symbol.iterator]() method.");',
    ],
    '@babel/runtime/helpers/createForOfIteratorHelper': [
        [
            /if\s?\(!\w+\s?&&\s?\w+\.return\s?!=\s?null\)\s?\w+\.return\(\)/,
            'throw new TypeError("Invalid attempt to iterate non-iterable instance.\\nIn order to be iterable, non-array objects must have a [Symbol.iterator]() method.");',
        ],
    ],
    '@babel/runtime/helpers/inherits': [
        'throw new TypeError("Super expression must either be null or a function")',
    ],
    '@babel/runtime/helpers/iterableToArray': [
        /if\s?\(typeof\sSymbol\s?!==\s?"undefined"\s?&&\s?\w+\[Symbol\.iterator\]\s?!=\s?null\s?\|\|\s?\w+\["@@iterator"\]\s?!=\s?null\)\s?return\sArray\.from\(\w+\)/,
    ],
    '@babel/runtime/helpers/iterableToArrayLimit': [
        /var \w=null==\w\?null:"undefined"!=typeof Symbol&&r\[Symbol\.iterator\]\|\|\w\["@@iterator"\]/,
    ],
    '@babel/runtime/helpers/iterableToArrayLimitLoose': [
        /var \w=\w\s?&&\s?\("undefined"\s?!=\s?typeof Symbol\s?&&\s?e\[Symbol\.iterator]\s?\|\|\s?\w\["@@iterator"\]\)/,
    ],
    '@babel/runtime/helpers/newArrowCheck': [
        'throw new TypeError("Cannot instantiate an arrow function")',
    ],
    '@babel/runtime/helpers/nonIterableRest': [
        'Invalid attempt to destructure non-iterable instance.\\nIn order to be iterable, non-array objects must have a [Symbol.iterator]() method.',
    ],
    '@babel/runtime/helpers/objectDestructuringEmpty': [
        'throw new TypeError("Cannot destructure "',
    ],
    '@babel/runtime/helpers/objectWithoutProperties': [
        [
            /if\s?\(\w+\.indexOf\(\w+\)\s?>=\s?0\)\s?continue/,
            /if\s?\(Object\.getOwnPropertySymbols\)\s?{(\r\n|\r|\n)?(\s+)?var \w+\s?=\s?Object\.getOwnPropertySymbols\(\w+\)/,
            /if\s?\(!Object\.prototype\.propertyIsEnumerable\.call\(\w+,\s?\w+\)\)\s?continue/,
        ],
    ],
    '@babel/runtime/helpers/objectWithoutPropertiesLoose': [
        [
            /if\s?\(\w+\.indexOf\(\w+\)\s?>=\s?0\)\s?continue/,
            /var \w+\s?=\s?{};?(\r\n|\r|\n)?(\s+)?var \w+\s?=\s?Object\.keys\(\w+\)/,
        ],
    ],
    '@babel/runtime/helpers/typeof': [
        /\w+\s?=\s?"function"\s?===?\s?typeof Symbol\s?&&\s?"symbol"\s?===?\s?typeof Symbol\.iterator\s?\?/,
        /&& \w+\.constructor\s?===?\s?Symbol\s?&&\s?\w+\s?!==?\s?Symbol\.prototype\s?\?\s?"symbol"\s?:\s?typeof\s?\w+/,
    ],
    '@babel/runtime/helpers/unsupportedIterableToArray': [
        /\/\^\(\?:Ui\|I\)nt\(\?:8\|16\|32\)\(\?:Clamped\)\?Array\$\/\.test\(\w+\)\)/,
    ],
    '@babel/runtime/helpers/wrapNativeSuper': [
        'throw new TypeError("Super expression must either be null or a function")',
    ],
}

const moduleDeps: Record<string, string[] | undefined> = {
    // '@babel/runtime/helpers/arrayLikeToArray': [],
    // '@babel/runtime/helpers/arrayWithHoles': [],
    '@babel/runtime/helpers/arrayWithoutHoles': [
        '@babel/runtime/helpers/arrayLikeToArray',
    ],
    // '@babel/runtime/helpers/classCallCheck': [],
    '@babel/runtime/helpers/construct': [
        '@babel/runtime/helpers/isNativeReflectConstruct',
        '@babel/runtime/helpers/setPrototypeOf',
    ],
    '@babel/runtime/helpers/createForOfIteratorHelper': [
        '@babel/runtime/helpers/unsupportedIterableToArray',
    ],
    '@babel/runtime/helpers/createForOfIteratorHelperLoose': [
        '@babel/runtime/helpers/unsupportedIterableToArray',
    ],
    '@babel/runtime/helpers/createSuper': [
        '@babel/runtime/helpers/getPrototypeOf',
        '@babel/runtime/helpers/isNativeReflectConstruct',
        '@babel/runtime/helpers/possibleConstructorReturn',
    ],
    '@babel/runtime/helpers/defineProperty': [
        '@babel/runtime/helpers/toPropertyKey',
    ],
    // '@babel/runtime/helpers/extends': [],
    '@babel/runtime/helpers/get': [
        '@babel/runtime/helpers/superPropBase',
    ],
    // '@babel/runtime/helpers/getPrototypeOf': [],
    '@babel/runtime/helpers/inherits': [
        '@babel/runtime/helpers/setPrototypeOf',
    ],
    '@babel/runtime/helpers/inheritsLoose': [
        '@babel/runtime/helpers/setPrototypeOf',
    ],
    // '@babel/runtime/helpers/isNativeFunction': [],
    // '@babel/runtime/helpers/isNativeReflectConstruct': [],
    // '@babel/runtime/helpers/iterableToArray': [],
    // '@babel/runtime/helpers/iterableToArrayLimit': [],
    // '@babel/runtime/helpers/iterableToArrayLimitLoose': [],
    '@babel/runtime/helpers/maybeArrayLike': [
        '@babel/runtime/helpers/arrayLikeToArray',
    ],
    // '@babel/runtime/helpers/newArrowCheck': [],
    // '@babel/runtime/helpers/nonIterableRest': [],
    // '@babel/runtime/helpers/nonIterableSpread': [],
    // '@babel/runtime/helpers/objectDestructuringEmpty': [],
    '@babel/runtime/helpers/objectSpread': [
        '@babel/runtime/helpers/defineProperty',
    ],
    '@babel/runtime/helpers/objectSpread2': [
        '@babel/runtime/helpers/defineProperty',
    ],
    '@babel/runtime/helpers/objectWithoutProperties': [
        '@babel/runtime/helpers/objectWithoutPropertiesLoose',
    ],
    // '@babel/runtime/helpers/objectWithoutPropertiesLoose': [],
    '@babel/runtime/helpers/set': [
        '@babel/runtime/helpers/defineProperty',
        '@babel/runtime/helpers/superPropBase',
    ],
    // '@babel/runtime/helpers/setPrototypeOf': [],
    // '@babel/runtime/helpers/skipFirstGeneratorNext': [],
    '@babel/runtime/helpers/slicedToArray': [
        '@babel/runtime/helpers/arrayWithHoles',
        '@babel/runtime/helpers/iterableToArrayLimit',
        '@babel/runtime/helpers/nonIterableRest',
        '@babel/runtime/helpers/unsupportedIterableToArray',
    ],
    '@babel/runtime/helpers/slicedToArrayLoose': [
        '@babel/runtime/helpers/arrayWithHoles',
        '@babel/runtime/helpers/iterableToArrayLimitLoose',
        '@babel/runtime/helpers/nonIterableRest',
        '@babel/runtime/helpers/unsupportedIterableToArray',
    ],
    '@babel/runtime/helpers/superPropBase': [
        '@babel/runtime/helpers/getPrototypeOf',
    ],
    // '@babel/runtime/helpers/taggedTemplateLiteral': [],
    // '@babel/runtime/helpers/taggedTemplateLiteralLoose': [],
    '@babel/runtime/helpers/toArray': [
        '@babel/runtime/helpers/arrayWithoutHoles',
        '@babel/runtime/helpers/iterableToArray',
        '@babel/runtime/helpers/nonIterableRest',
        '@babel/runtime/helpers/unsupportedIterableToArray',
    ],
    '@babel/runtime/helpers/toConsumableArray': [
        '@babel/runtime/helpers/arrayWithoutHoles',
        '@babel/runtime/helpers/iterableToArray',
        '@babel/runtime/helpers/nonIterableSpread',
        '@babel/runtime/helpers/unsupportedIterableToArray',
    ],
    // '@babel/runtime/helpers/toPrimitive': [],
    '@babel/runtime/helpers/toPropertyKey': [
        '@babel/runtime/helpers/toPrimitive',
    ],
    '@babel/runtime/helpers/unsupportedIterableToArray': [
        '@babel/runtime/helpers/arrayLikeToArray',
    ],
    '@babel/runtime/helpers/wrapNativeSuper': [
        '@babel/runtime/helpers/construct',
        '@babel/runtime/helpers/getPrototypeOf',
        '@babel/runtime/helpers/isNativeFunction',
        '@babel/runtime/helpers/setPrototypeOf',
    ],
}

export function scanBabelRuntime(j: JSCodeshift, module: Module) {
    const root = module.ast
    const statements = getTopLevelStatements(root)
    const functions = statements.filter((node): node is FunctionExpression | FunctionDeclaration | ArrowFunctionExpression => {
        return j.FunctionDeclaration.check(node)
            || j.ArrowFunctionExpression.check(node)
    })

    functions.forEach((func) => {
        const functionName = func.id?.name
        if (!functionName || typeof functionName !== 'string') return

        const code = j(func).toSource()

        const collectedTags = new Set(Object.keys(moduleMatchers)
            .filter((moduleName) => {
                const matchers = moduleMatchers[moduleName]
                return matchers.some((matcher) => {
                    if (typeof matcher === 'string') {
                        return code.includes(matcher)
                    }
                    else if (matcher instanceof RegExp) {
                        return matcher.test(code)
                    }
                    else if (Array.isArray(matcher)) {
                        return matcher.every((m) => {
                            if (typeof m === 'string') {
                                return code.includes(m)
                            }
                            else if (m instanceof RegExp) {
                                return m.test(code)
                            }
                            return false
                        })
                    }
                    return false
                })
            }),
        )

        /**
         * Module's dependencies might be inlined by compiler.
         * So we need to remove scanned tag that are dependent
         * of other scanned tags.
         */
        const _collectedTags = [...collectedTags]
        const tagsDependencies = _collectedTags.flatMap(tag => moduleDeps[tag] ?? [])
        const tags = _collectedTags.filter(tag => !tagsDependencies.includes(tag))

        module.tags[functionName] ??= []
        module.tags[functionName].push(...tags)
    })
}
