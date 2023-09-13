import { isNull, isStringLiteral, isTrue, isUndefined, nonNull } from '../utils/checker'
import { removePureAnnotation } from '../utils/comments'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { ASTNode, CallExpression, JSCodeshift, JSXElement, JSXExpressionContainer, JSXFragment, JSXIdentifier, JSXMemberExpression, JSXSpreadChild, JSXText, SpreadElement } from 'jscodeshift'

interface Params {
    pragma?: string
    pragmaFrag?: string
}

const DEFAULT_PRAGMA_CANDIDATES = [
    'createElement', // React: runtime = "classic" (`React.createElement`)
    'jsx', // React: runtime = "automatic" (`jsxRuntime.jsx`)
    '_jsx', // `import { jsx as _jsx } from 'react/jsx-runtime'`
    'h', // Preact
]

const DEFAULT_PRAGMA_FRAG_CANDIDATES = [
    'Fragment', // React
]

/**
 * Converts `React.createElement` to JSX.
 */
export const transformAST: ASTTransformation<Params> = (context, params) => {
    const { root, j } = context

    let pragmas = DEFAULT_PRAGMA_CANDIDATES
    if (params.pragma) {
        if (params.pragma.includes('.')) {
            // React.createElement -> createElement
            const [_object, property] = params.pragma.split('.')
            pragmas = [property]
        }
        else {
            pragmas = [params.pragma]
        }
    }

    let pragmaFrags = DEFAULT_PRAGMA_FRAG_CANDIDATES
    if (params.pragmaFrag) {
        if (params.pragmaFrag.includes('.')) {
            // React.Fragment -> Fragment
            const [_object, property] = params.pragmaFrag.split('.')
            pragmaFrags = [property]
        }
        else {
            pragmaFrags = [params.pragmaFrag]
        }
    }

    root
        .find(j.CallExpression, {
            callee: (callee: CallExpression['callee']) => {
                if (j.Identifier.check(callee)) {
                    return pragmas.includes(callee.name)
                }

                if (
                    j.MemberExpression.check(callee)
                    && j.Identifier.check(callee.object)
                    && j.Identifier.check(callee.property)
                ) {
                    return pragmas.includes(callee.property.name)
                }
                return false
            },
        })
        .paths()
        .reverse()
        .forEach((path) => {
            const jsxElement = toJSX(j, path.node, pragmaFrags)
            if (jsxElement) {
                const parentWithComments = j.ExpressionStatement.check(path.parent.node) ? path.parent : path
                removePureAnnotation(j, parentWithComments.node)

                path.replace(jsxElement)
            }
        })
}

function toJSX(j: JSCodeshift, node: ASTNode, pragmaFrags: string[]): JSXElement | JSXExpressionContainer | JSXFragment | JSXSpreadChild | JSXText | null {
    if (
        j.JSXElement.check(node)
     || j.JSXFragment.check(node)
     || j.JSXText.check(node)
     || j.JSXExpressionContainer.check(node)
     || j.JSXSpreadChild.check(node)
    ) {
        return node
    }

    if (j.Literal.check(node)) {
        if (typeof node.value === 'string') {
            const textContent = node.value
            // if contains invalid characters like {, }, <, >, \r, \n
            // then wrap it with jsxExpressionContainer `{textContent}`
            const shouldEscape = /[{}<>\r\n]/.test(textContent)
            return shouldEscape
                ? j.jsxExpressionContainer(node)
                : j.jsxText(textContent)
        }

        /**
         * empty nodes (null, undefined, true, and false)
         */
        if (typeof node.value === 'boolean' || node.value === null) {
            return null
        }

        return j.jsxExpressionContainer(node)
    }

    if (j.SpreadElement.check(node)) {
        return j.jsxSpreadChild(node.argument)
    }

    if (
        j.Identifier.check(node)
    || j.MemberExpression.check(node)
    || j.ObjectExpression.check(node)
    || j.ArrayExpression.check(node)
    ) {
        if (isUndefined(j, node)) return null

        return j.jsxExpressionContainer(node)
    }

    if (j.CallExpression.check(node)) {
        const args = node.arguments
        const type = args[0]
        const props = args[1]

        const tag = toJsxTag(j, type) as JSXIdentifier | JSXMemberExpression | null
        if (!tag) return null

        const attributes = toJsxAttributes(j, props)

        const childrenArgs = args.slice(2)
        const children = childrenArgs.map(child => toJSX(j, child, pragmaFrags)).filter(nonNull)

        /**
         * Post-processing children:
         *
         * The semantics of concatenating adjacent JSXText is ambiguous,
         * it can be `foobar`, `foo bar` or `foo\nbar`.
         * We choose the spaced version for better readability.
         */
        if (children.length > 1) {
            for (let i = 1; i < children.length; i++) {
                const child = children[i]
                const prevChild = children[i - 1]
                if (j.JSXText.check(child) && j.JSXText.check(prevChild)) {
                    prevChild.value += ` ${child.value}`
                    children.splice(i, 1)
                    i--
                }
            }
        }

        if (attributes.length === 0) {
            const isFrag1 = j.JSXIdentifier.check(tag) && pragmaFrags.includes(tag.name)
            const isFrag2 = j.JSXMemberExpression.check(tag) && pragmaFrags.includes(tag.property.name)
            if (isFrag1 || isFrag2) {
                return j.jsxFragment(j.jsxOpeningFragment(), j.jsxClosingFragment(), children)
            }
        }

        const openingElement = j.jsxOpeningElement(tag, attributes)
        const closingElement = j.jsxClosingElement(tag)
        const selfClosing = children.length === 0
        if (selfClosing) openingElement.selfClosing = true
        const jsxElement = j.jsxElement(openingElement, selfClosing ? null : closingElement, children)

        return jsxElement
    }

    return null
}

function toJsxTag(j: JSCodeshift, node: ASTNode): JSXElement | JSXExpressionContainer | JSXMemberExpression | JSXText | JSXIdentifier | null {
    if (j.Literal.check(node) && typeof node.value === 'string') {
        return j.jsxIdentifier(node.value)
    }
    else if (j.Identifier.check(node)) {
        return j.jsxIdentifier(node.name)
    }
    else if (j.MemberExpression.check(node)) {
        return j.jsxMemberExpression(
            toJsxTag(j, node.object) as JSXIdentifier | JSXMemberExpression,
            toJsxTag(j, node.property) as JSXIdentifier,
        )
    }

    return null
}

function toJsxAttributes(j: JSCodeshift, props: SpreadElement | ExpressionKind) {
    const attributes = []
    if (j.ObjectExpression.check(props)) {
        const properties = props.properties.map((prop) => {
            if (j.SpreadElement.check(prop) || j.SpreadProperty.check(prop)) {
                return j.jsxSpreadAttribute(prop.argument)
            }

            // method(a) {...} -> method={{a} => {...}}
            if (j.ObjectMethod.check(prop)) {
                if (!j.Identifier.check(prop.key)) {
                    console.warn(`[un-jsx] unsupported attribute: ${j(prop).toSource()}`)
                    return null
                }

                const name = prop.key
                const value = j.arrowFunctionExpression(prop.params, prop.body)
                return j.jsxAttribute(j.jsxIdentifier(name.name), j.jsxExpressionContainer(value))
            }

            const name = prop.key
            const value = prop.value

            if (
                j.RestElement.check(value)
            || j.PropertyPattern.check(value)
            || j.ObjectPattern.check(value)
            || j.ArrayPattern.check(value)
            || j.AssignmentPattern.check(value)
            || j.TSParameterProperty.check(value)
            || j.SpreadElement.check(value)
            || j.SpreadProperty.check(value)
            || j.SpreadElementPattern.check(value)
            || j.SpreadPropertyPattern.check(value)
            ) {
                console.warn(`[un-jsx] unsupported attribute: ${j(prop).toSource()}`)
                return null
            }

            if (prop.computed) {
                const property = j.objectProperty(name, value)
                property.computed = true
                const obj = j.objectExpression([property])
                return j.jsxSpreadAttribute(obj)
            }

            if (j.Identifier.check(name) || isStringLiteral(j, name)) {
                const k = j.Identifier.check(name)
                    ? j.jsxIdentifier(name.name)
                    : j.jsxIdentifier(name.value)
                if (isTrue(j, value)) return j.jsxAttribute(k)

                const v = isStringLiteral(j, value)
                    ? value
                    : j.jsxExpressionContainer(value)
                return j.jsxAttribute(k, v)
            }

            // unsupported
            console.warn(`[un-jsx] unsupported attribute: ${j(prop).toSource()}`)
            return null
        }).filter(nonNull)
        attributes.push(...properties)
    }
    else if (isNull(j, props)) {
        // empty props
    }
    else if (!j.SpreadElement.check(props) && !j.SpreadProperty.check(props)) {
        attributes.push(j.jsxSpreadAttribute(props))
    }

    return attributes
}

export default wrap(transformAST)
