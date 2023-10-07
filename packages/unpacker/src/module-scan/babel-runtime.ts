import { isTopLevel } from '@unminify-kit/ast-utils'
import type { Module } from '../Module'
import type { ArrowFunctionExpression, FunctionDeclaration, FunctionExpression, JSCodeshift, Statement } from 'jscodeshift'

const moduleMatchers: Record<string, Array<string | RegExp | Array<string | RegExp>>> = {
    '@babel/runtime/helpers/arrayLikeToArray': [
        /for\s?\(var \w+\s?=\s?0,\s?\w+\s?=\s?(new\s)?Array\(\w+\);\s?\w+\s?<\s?\w+;\s?\w+\+\+\)\s?\w+\[\w+\]\s?=\s?\w+\[\w+\]/,
    ],
    '@babel/runtime/helpers/arrayWithHoles': [
        /{(\r\n|\r|\n)?(\s+)?if\s?\(Array\.isArray\(\w+\)\)\s?return\s?\w+;?(\r\n|\r|\n)?(\s+)?}/,
    ],
    '@babel/runtime/helpers/classCallCheck': [
        'throw new TypeError("Cannot call a class as a function")',
    ],
    '@babel/runtime/helpers/createForOfIteratorHelperLoose': [
        'throw new TypeError("Invalid attempt to iterate non-iterable instance.\\nIn order to be iterable, non-array objects must have a [Symbol.iterator]() method.")',
    ],
    '@babel/runtime/helpers/createForOfIteratorHelper': [
        [
            /if\s?\(!\w+\s?&&\s?\w+\.return\s?!=\s?null\)\s?\w+\.return\(\)/,
            'throw new TypeError("Invalid attempt to iterate non-iterable instance.\\nIn order to be iterable, non-array objects must have a [Symbol.iterator]() method.")',
        ],
    ],
    '@babel/runtime/helpers/extends': [
        /\w+\s?=\s?Object\.assign\s?\|\|\s?function\s?\(\w+\)/,
        // v7.18.2 added a .bind() call to Object.assign
        /\w+\s?=\s?Object\.assign\s?\?\s?Object\.assign\.bind\(\)\s?:\s?function\s?\(\w+\)/,
    ],
    '@babel/runtime/helpers/inherits': [
        'throw new TypeError("Super expression must either be null or a function")',
    ],
    '@babel/runtime/helpers/interopRequireDefault': [
        /return\s?\w+\s?&&\s?\w+\.__esModule\s?\?\s?\w+\s?:\s?{ default: \w+ }/,
    ],
    '@babel/runtime/helpers/interopRequireWildcard': [
        'typeof WeakMap',
        // !nodeInterop && obj && obj.__esModule
        /!\w+\s?&&\s?\w+\s?&&\s?\w+\.__esModule/,
    ],
    '@babel/runtime/helpers/iterableToArray': [
        /if\s?\(typeof\sSymbol\s?!==\s?"undefined"\s?&&\s?\w+\[Symbol\.iterator\]\s?!=\s?null\s?\|\|\s?\w+\["@@iterator"\]\s?!=\s?null\)\s?return\sArray\.from\(\w+\)/,
    ],
    '@babel/runtime/helpers/iterableToArrayLimit': [
        /\w+\s?=\s?null\s?==\s?\w+\s?\?\s?null\s?:\s?"undefined"\s?!=\s?typeof Symbol\s?&&\s?\w+\[Symbol\.iterator\]\s?\|\|\s?\w+\["@@iterator"\]/,
    ],
    '@babel/runtime/helpers/iterableToArrayLimitLoose': [
        /\w+\s?=\s?\w+\s?&&\s?\("undefined"\s?!=\s?typeof Symbol\s?&&\s?\w+\[Symbol\.iterator]\s?\|\|\s?\w+\["@@iterator"\]\)/,
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
        [
            // let's try some keyword based matching
            '.indexOf(',
            'Object.getOwnPropertySymbols',
            'Object.prototype.propertyIsEnumerable.call(',
            /\w+\[\w+\]\s?=\s?\w+\[\w+\]/,
        ],
    ],
    // FIXME: this function's implementation is too generic, we need to find a better way to match it.
    '@babel/runtime/helpers/objectWithoutPropertiesLoose': [
        [
            /if\s?\(\w+\.indexOf\(\w+\)\s?>=\s?0\)\s?continue/,
            /var \w+\s?=\s?{};?(\r\n|\r|\n)?(\s+)?var \w+\s?=\s?Object\.keys\(\w+\)/,
            /\w+\[\w+\]\s?=\s?\w+\[\w+\]/,
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
    // '@babel/runtime/helpers/interopRequireDefault': [],
    // '@babel/runtime/helpers/interopRequireWildcard': [],
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

/**
 * Scan all top level functions and mark tags based on the content of the function.
 */
export function scanBabelRuntime(j: JSCodeshift, module: Module) {
    const root = module.ast
    const statements = root.get().node.body as Statement[]
    const functions = statements.filter((node): node is FunctionExpression | FunctionDeclaration | ArrowFunctionExpression => {
        return j.FunctionDeclaration.check(node)
            || j.ArrowFunctionExpression.check(node)
    })

    functions.forEach((func) => {
        const functionName = func.id?.name
        if (!functionName || typeof functionName !== 'string') return

        const code = j(func).toSource()

        const collectedTags = [...new Set(Object.keys(moduleMatchers)
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
        )]

        /**
         * Module's dependencies might be inlined by compiler.
         * So we need to remove scanned tag that are dependent
         * of other scanned tags.
         */
        const tagsDependencies = collectedTags.flatMap(tag => moduleDeps[tag] ?? [])
        const tags = collectedTags.filter(tag => !tagsDependencies.includes(tag))

        module.tags[functionName] ??= []
        module.tags[functionName].push(...tags)
    })
}

/**
 * Go through all tagged functions and check for the usage of other tagged functions.
 * If a tagged function is used, then we need to add the tag to the function.
 * In the end, we will have a complete list of tags for each function, so that
 * we have a chance to change the tags to their upper level functions.
 */
export function postScanBabelRuntime(j: JSCodeshift, modules: Module[]) {
    modules.forEach((module) => {
        const { ast: root, import: imports } = module
        const rootScope = root.get().scope

        const taggedImportLocals = new Map<string, string[]>()
        imports.forEach((imp) => {
            if (imp.type === 'bare' || imp.type === 'namespace') return

            const targetModule = modules.find(m => m.id.toString() === imp.source.toString())
            if (!targetModule || Object.keys(targetModule.tags).length === 0) return

            if (imp.type === 'named') {
                const targetTags = targetModule.tags[imp.name]
                if (!targetTags || targetTags.length === 0) return
                taggedImportLocals.set(imp.local, targetTags)
                return
            }

            if (imp.type === 'default') {
                const targetTags = targetModule.tags.default
                if (targetTags && targetTags.length !== 0) {
                    taggedImportLocals.set(imp.name, targetTags)
                }

                Object.entries(targetModule.export).forEach(([exportName, exportLocalName]) => {
                    const targetTags = targetModule.tags[exportLocalName]
                    // TODO: Currently we didn't pull in dependent's imported tags.
                    // We might need to build a module graph and start from the leaf.
                    if (!targetTags || targetTags.length === 0) return
                    taggedImportLocals.set(`${imp.name}.${exportName}`, targetTags)
                })
            }
        }, {} as Record<string, string[]>)

        const functionPaths = [
            ...root.find(j.FunctionDeclaration).filter(p => isTopLevel(j, p)).paths(),
            ...root.find(j.ArrowFunctionExpression).filter(p => isTopLevel(j, p)).paths(),
        ]

        functionPaths.forEach((func) => {
            const functionName = func.node.id?.name
            if (!functionName || typeof functionName !== 'string') return

            taggedImportLocals.forEach((tags, localName) => {
                const [importObj, importProp] = localName.split('.')
                const isReferenced = localName.includes('.')
                    ? j(func)
                        .find(j.MemberExpression, {
                            object: { name: importObj },
                            property: { name: importProp },
                        })
                        .filter((path) => {
                            const scope = path.scope?.lookup(importObj)
                            return scope === rootScope
                        })
                        .size() > 0
                    : j(func)
                        .find(j.Identifier, { name: localName })
                        .filter((path) => {
                            const scope = path.scope?.lookup(localName)
                            return scope === rootScope
                        })
                        .size() > 0

                if (isReferenced) {
                    module.tags[functionName] ??= []
                    module.tags[functionName].push(...tags)
                }
            })

            if (module.tags[functionName]?.length === 0) return

            /**
             * Try to combine tags based on dependencies.
             */
            const moduleTag = module.tags[functionName]!
            let score = 0
            let matchedTag: string | null = null
            Object.entries(moduleDeps).forEach(([tag, deps]) => {
                if (!deps) return
                const allMatch = deps.every((dep) => {
                    return moduleTag.includes(dep)
                })
                // TODO: we can further improve the scoring algorithm.
                if (allMatch && deps.length > score) {
                    score = deps.length
                    matchedTag = tag
                }
            })
            if (matchedTag) {
                const deps = moduleDeps[matchedTag]!
                deps.forEach((dep) => {
                    const index = moduleTag.indexOf(dep)
                    moduleTag.splice(index, 1)
                })
                moduleTag.unshift(matchedTag, ...deps.map(dep => `- ${dep}`))
            }
        })
    })
}
