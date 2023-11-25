import { generateName, isDeclared, renameIdentifier } from '@wakaru/ast-utils'
import { assertScopeExists } from '../utils/assert'
import { pascalCase } from '../utils/case'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { Scope } from 'ast-types/lib/scope'
import type { ASTPath, ArrayPattern, ArrowFunctionExpression, CallExpression, ClassMethod, Collection, FunctionDeclaration, FunctionExpression, Identifier, JSCodeshift, ObjectMethod, ObjectPattern } from 'jscodeshift'

const MINIFIED_IDENTIFIER_THRESHOLD = 2

/**
 * Rename minified identifiers with heuristic rules.
 *
 * @example
 * let { gql: t, dispatchers: o, listener: i } = n;
 * o.delete(t, i);
 * ->
 * let { gql, dispatchers, listener } = n;
 * dispatchers.delete(mql, listener);
 *
 * @TODO
 * const I = e.atom,
 * export default {
 *   themeAtom: I,
 * };
 * ->
 * const themeAtom = e.atom,
 * export default {
 *  themeAtom,
 * };
 *
 * @example React ecosystem name guessing
 * const d = o.createContext(u);
 * ->
 * const uContext = o.createContext(u);
 *
 * const [e, f] = o.useState(0);
 * ->
 * const [e, SetE] = o.useState(0);
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    handleDestructuringRename(j, root)
    handleReactRename(j, root)
}

/**
 * let { gql: t, dispatchers: o, listener: i } = n;
 * o.delete(t, i);
 * ->
 * let { gql, dispatchers, listener } = n;
 * dispatchers.delete(mql, listener);
 */
function handleDestructuringRename(j: JSCodeshift, root: Collection) {
    root
        .find(j.VariableDeclarator, { id: { type: 'ObjectPattern' } })
        .forEach((path) => {
            const scope = path.scope
            assertScopeExists(scope)

            const id = path.node.id as ObjectPattern
            handlePropertyRename(j, id, scope)
        })

    root
        .find(j.FunctionDeclaration)
        .forEach(path => handleFunctionParamsRename(j, path))

    root
        .find(j.ArrowFunctionExpression)
        .forEach(path => handleFunctionParamsRename(j, path))

    root
        .find(j.FunctionExpression)
        .forEach(path => handleFunctionParamsRename(j, path))

    root
        .find(j.ObjectMethod)
        .forEach(path => handleFunctionParamsRename(j, path))

    root
        .find(j.ClassMethod)
        .forEach(path => handleFunctionParamsRename(j, path))
}

function handleFunctionParamsRename(j: JSCodeshift, path: ASTPath<FunctionDeclaration | FunctionExpression | ArrowFunctionExpression | ObjectMethod | ClassMethod>) {
    const scope = path.scope
    assertScopeExists(scope)

    path.node.params.forEach(param => j.ObjectPattern.check(param) && handlePropertyRename(j, param, scope))
}

function handlePropertyRename(j: JSCodeshift, objectPattern: ObjectPattern, scope: Scope) {
    objectPattern.properties.forEach((property) => {
        if (!j.ObjectProperty.check(property)) return
        if (property.computed || property.shorthand) return

        const key = property.key
        if (!j.Identifier.check(key)) return

        const value = j.AssignmentPattern.check(property.value) ? property.value.left : property.value
        if (!j.Identifier.check(value)) return

        // If the key is longer than the value, rename the value
        if (key.name.length > value.name.length) {
            if (isDeclared(scope, key.name)) return

            renameIdentifier(j, scope, value.name, key.name)
            property.shorthand = key.name === value.name
        }
    })
}

function handleReactRename(j: JSCodeshift, root: Collection) {
    /**
     * const d = o.createContext(u);
     * ->
     * const uContext = o.createContext(u);
     */
    root
        .find(j.VariableDeclarator, {
            id: { type: 'Identifier' },
            init: { type: 'CallExpression' },
        })
        .forEach((path) => {
            const scope = path.scope
            assertScopeExists(scope)

            const id = path.node.id as Identifier
            const init = path.node.init as CallExpression

            if (id.name.length > MINIFIED_IDENTIFIER_THRESHOLD) return

            const callee = init.callee
            const calleeName = getElementName(j, callee)
            if (!calleeName.endsWith('.createContext') && calleeName !== 'createContext') return

            const args = init.arguments
            if (args.length > 1) return

            // rename the identifier
            const oldName = id.name
            const newName = generateName(`${pascalCase(oldName)}Context`, scope)
            renameIdentifier(j, scope, oldName, newName)
        })

    /**
     * const d = o.useRef(u);
     * ->
     * const uRef = o.useRef(u);
     */
    root
        .find(j.VariableDeclarator, {
            id: { type: 'Identifier' },
            init: { type: 'CallExpression' },
        })
        .forEach((path) => {
            const scope = path.scope
            assertScopeExists(scope)

            const id = path.node.id as Identifier
            const init = path.node.init as CallExpression

            if (id.name.length > MINIFIED_IDENTIFIER_THRESHOLD) return

            const callee = init.callee
            const calleeName = getElementName(j, callee)
            if (!calleeName.endsWith('.useRef') && calleeName !== 'useRef') return

            const args = init.arguments
            if (args.length > 1) return

            // rename the identifier
            const oldName = id.name
            const newName = generateName(`${pascalCase(oldName)}Ref`, scope)
            renameIdentifier(j, scope, oldName, newName)
        })

    /**
     * const [e, f] = o.useState(0);
     * ->
     * const [e, SetE] = o.useState(0);
     */
    root
        .find(j.VariableDeclarator, {
            id: { type: 'ArrayPattern' },
            init: { type: 'CallExpression' },
        })
        .forEach((path) => {
            const scope = path.scope
            assertScopeExists(scope)

            const id = path.node.id as ArrayPattern
            if (!id.elements || id.elements.length === 0 || id.elements.length > 2) return
            if (!j.Identifier.check(id.elements[0]) && id.elements[0] !== null) return
            if (!j.Identifier.check(id.elements[1])) return

            const init = path.node.init as CallExpression
            const callee = init.callee
            const calleeName = getElementName(j, callee)
            if (!calleeName.endsWith('.useState') && calleeName !== 'useState') return

            const args = init.arguments
            if (args.length > 1) return

            // rename the identifier
            const stateName = id.elements[0]?.name
            const setStateName = id.elements[1].name
            const baseName = stateName || setStateName
            if (baseName.length > MINIFIED_IDENTIFIER_THRESHOLD) return
            const newName = generateName(`set${pascalCase(baseName)}`, scope)
            renameIdentifier(j, scope, setStateName, newName)
        })
}

/**
 * Returns the element name of a MemberExpression or Identifier.
 * For example:
 *   getElementName(j, a.b.c) -> a.b.c
 *   getElementName(j, a) -> a
 */
function getElementName(j: JSCodeshift, node: ExpressionKind): string {
    if (j.Identifier.check(node)) return node.name
    if (j.StringLiteral.check(node)) return node.value

    if (j.MemberExpression.check(node)) {
        return `${getElementName(j, node.object)}.${getElementName(j, node.property)}`
    }

    return '[unknown]'
}

export default wrap(transformAST)
