import { isNull, isStringLiteral, isTrue, isUndefined, nonNull } from '../utils/checker'
import { removePureAnnotation } from '../utils/comments'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { CallExpression, JSCodeshift, JSXAttribute, JSXElement, JSXExpressionContainer, JSXFragment, JSXIdentifier, JSXMemberExpression, JSXSpreadAttribute, JSXSpreadChild, JSXText, SpreadElement } from 'jscodeshift'

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

function toJSX(j: JSCodeshift, node: CallExpression, pragmaFrags: string[]): JSXElement | JSXFragment | null {
    const [type, props, ...childrenArgs] = node.arguments

    const tag = toJsxTag(j, type)
    if (!tag) return null

    const attributes = toJsxAttributes(j, props)

    const children = postProcessChildren(j, childrenArgs.map(child => toJsxChild(j, child)).filter(nonNull))

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

    return j.jsxElement(openingElement, selfClosing ? null : closingElement, children)
}

function toJsxTag(j: JSCodeshift, node: SpreadElement | ExpressionKind): JSXIdentifier | JSXMemberExpression | null {
    if (j.Literal.check(node) && typeof node.value === 'string') {
        return j.jsxIdentifier(node.value)
    }
    // TODO: namespace
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

const canLiteralBePropString = (node: any) => {
    return !node.raw.includes('\\') && !node.value.includes('"')
}

function toJsxAttributes(j: JSCodeshift, props: SpreadElement | ExpressionKind): Array<JSXAttribute | JSXSpreadAttribute> {
    // null means empty props
    if (isNull(j, props)) return []

    /**
     * `React.__spread` is deprecated since React v15.0.1
     * https://ru.legacy.reactjs.org/blog/2016/04/08/react-v15.0.1.html
     *
     * Copied from https://github.com/reactjs/react-codemod/blob/b34b92a1f0b8ad333efe5effb50d17d46d66588b/transforms/create-element-to-jsx.js#L30
     */
    const isReactSpread = j.CallExpression.check(props)
        && j.MemberExpression.check(props.callee)
        && j.Identifier.check(props.callee.object)
        // && props.callee.object.name === 'React'
        && j.Identifier.check(props.callee.property)
        && props.callee.property.name === '__spread'

    const isObjectAssign = j.CallExpression.check(props)
        && j.MemberExpression.check(props.callee)
        && j.Identifier.check(props.callee.object)
        && props.callee.object.name === 'Object'
        && j.Identifier.check(props.callee.property)
        && props.callee.property.name === 'assign'

    /**
     * Other spread syntax might be transformed to `__assign` or `__spread` by Babel.
     * They will be handled by other rules.
     */
    if (isReactSpread || isObjectAssign) {
        return props.arguments.map(arg => toJsxAttributes(j, arg)).flat()
    }

    if (j.ObjectExpression.check(props)) {
        return props.properties.map((prop) => {
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

                const v = isStringLiteral(j, value) && canLiteralBePropString(value)
                    ? value
                    : j.jsxExpressionContainer(value)
                return j.jsxAttribute(k, v)
            }

            // unsupported
            console.warn(`[un-jsx] unsupported attribute: ${j(prop).toSource()}`)
            return null
        }).filter(nonNull)
    }

    if (j.SpreadElement.check(props) || j.SpreadProperty.check(props)) {
        return toJsxAttributes(j, props.argument)
    }

    return [j.jsxSpreadAttribute(props)]
}

function toJsxChild(j: JSCodeshift, node: SpreadElement | ExpressionKind) {
    // Skip existing jsx nodes
    if (
        j.JSXElement.check(node)
     || j.JSXFragment.check(node)
     || j.JSXText.check(node)
     || j.JSXExpressionContainer.check(node)
     || j.JSXSpreadChild.check(node)
    ) {
        return node
    }

    // undefined is empty node
    if (isUndefined(j, node)) return null

    if (j.Literal.check(node)) {
        // null and bool are empty node
        if (typeof node.value === 'boolean' || node.value === null) {
            return null
        }

        if (typeof node.value === 'string') {
            const textContent = node.value
            const notEmpty = textContent !== ''
            // if contains invalid characters like {, }, <, >, \r, \n
            const needEscape = /[{}<>\r\n]/.test(textContent)
            // if contains whitespace at the beginning or end
            const needTrim = /^\s|\s$/.test(textContent)

            if (notEmpty && !needEscape && !needTrim) return j.jsxText(textContent)
        }

        return j.jsxExpressionContainer(node)
    }

    if (j.SpreadElement.check(node)) {
        return j.jsxSpreadChild(node.argument)
    }

    return j.jsxExpressionContainer(node)
}

/**
 * Add text newline nodes between children so recast formats
 * one child per line instead of all children on one line.
 *
 * See: https://github.com/reactjs/react-codemod/blob/b34b92a1f0b8ad333efe5effb50d17d46d66588b/transforms/create-element-to-jsx.js#L227C7-L227C81
 */
function postProcessChildren(j: JSCodeshift, children: Array<JSXExpressionContainer | JSXElement | JSXFragment | JSXText | JSXSpreadChild>) {
    const lineBreak = j.jsxText('\n')
    if (children.length > 0) {
        if (children.length === 1 && j.JSXText.check(children[0])) {
            return children
        }
        return [lineBreak, ...children.flatMap(child => [child, lineBreak])]
    }
    return children
}

export default wrap(transformAST)
