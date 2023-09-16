import { generateName } from '../utils/identifier'
import { nonNull } from '../utils/utils'
import wrap from '../wrapAstTransformation'
import type { ASTTransformation } from '../wrapAstTransformation'
import type { StatementKind } from 'ast-types/lib/gen/kinds'
import type { Scope } from 'ast-types/lib/scope'
import type { ExpressionStatement, Identifier, JSCodeshift, Literal, MemberExpression, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

/**
 * Converts object property accesses and array index accesses to destructuring.
 *
 * @example
 * const t = e.x;
 * const n = e.y;
 * const r = e.color;
 * console.log(t, n, r);
 * ->
 * const { x, y, color } = e;
 * console.log(x, y, color);
 *
 * @example
 * const t = e[0];
 * const n = e[1];
 * const r = e[2];
 * console.log(t, n, r);
 * ->
 * const [t, n, r] = e;
 * console.log(t, n, r);
 */
export const transformAST: ASTTransformation = (context) => {
    const { root, j } = context

    root
        .find(j.BlockStatement)
        .forEach((path) => {
            const { body } = path.node
            const scope = path.scope
            if (!scope) return
            handleSmartInline(j, body, scope)
        })

    root
        .find(j.Program)
        .forEach((path) => {
            const { body } = path.node
            const scope = path.scope
            if (!scope) return
            handleSmartInline(j, body, scope)
        })
}

function handleSmartInline(j: JSCodeshift, body: StatementKind[], scope: Scope) {
    const objectPropertyMap = new Map<string, Set<string>>()
    const objectDeclarationMap = new Map<string, Array<VariableDeclaration | ExpressionStatement>>()

    const objectIndexMap = new Map<string, string[]>()
    const variableDeclarationMap = new Map<string, VariableDeclaration>()

    body.forEach((node) => {
        // Collect all object property accesses
        if (j.match(node, {
            type: 'VariableDeclaration',
            declarations: [{
                type: 'VariableDeclarator',
                // @ts-expect-error
                id: {
                    type: 'Identifier',
                },
                init: {
                    type: 'MemberExpression',
                    // @ts-expect-error
                    object: {
                        type: 'Identifier',
                    },
                    // @ts-expect-error
                    property: {
                        type: 'Identifier',
                    },
                },
            }],
        })) {
            const _node = node as VariableDeclaration
            const declarations = _node.declarations
            if (declarations.length !== 1) return

            const variableDeclarator = declarations[0] as VariableDeclarator
            const init = variableDeclarator.init as MemberExpression
            if (init.computed) return

            const object = init.object as Identifier
            const property = init.property as Identifier

            const propertyAccesses = objectPropertyMap.get(object.name) || new Set()
            propertyAccesses.add(property.name)
            objectPropertyMap.set(object.name, propertyAccesses)

            const variableDeclarations = objectDeclarationMap.get(object.name) || []
            variableDeclarations.push(_node)
            objectDeclarationMap.set(object.name, variableDeclarations)
        }

        // Collect all index accesses
        if (j.match(node, {
            type: 'VariableDeclaration',
            declarations: [{
                type: 'VariableDeclarator',
                // @ts-expect-error
                id: {
                    type: 'Identifier',
                },
                init: {
                    type: 'MemberExpression',
                    // @ts-expect-error
                    object: {
                        type: 'Identifier',
                    },
                    // @ts-expect-error
                    property: {
                        type: 'Literal',
                    },
                },
            }],
        })) {
            const _node = node as VariableDeclaration
            const declarations = _node.declarations
            if (declarations.length !== 1) return

            const variableDeclarator = declarations[0] as VariableDeclarator
            const init = variableDeclarator.init as MemberExpression
            if (!init.computed) return

            const id = variableDeclarator.id as Identifier
            const object = init.object as Identifier
            const property = init.property as Literal
            const index = property.value
            // if the index is too large, the generated code will be too long or weird
            if (typeof index !== 'number' || index > 10) return

            const indexAccesses = objectIndexMap.get(object.name) || []
            indexAccesses[index] = id.name
            objectIndexMap.set(object.name, indexAccesses)
            variableDeclarationMap.set(id.name, _node)
        }

        /**
         * Property access in expression statement is considered
         * as part of the destructuring. But normally people don't
         * write code like this, so why we do this?
         *
         * When a destructuring variable is not used, bundler will
         * transform it to a comma expression, which then will be
         * splitted by rule `un-sequence-expression`. That's why we
         * see this pattern IRL.
         */
        if (j.match(node, {
            type: 'ExpressionStatement',
            expression: {
                type: 'MemberExpression',
                // @ts-expect-error
                object: {
                    type: 'Identifier',
                },
                // @ts-expect-error
                property: {
                    type: 'Identifier',
                },
            },
        })) {
            const _node = node as ExpressionStatement
            const expression = _node.expression as MemberExpression
            if (expression.computed) return

            const object = expression.object as Identifier
            const property = expression.property as Identifier

            const propertyAccesses = objectPropertyMap.get(object.name) || new Set()
            propertyAccesses.add(property.name)
            objectPropertyMap.set(object.name, propertyAccesses)

            const variableDeclarations = objectDeclarationMap.get(object.name) || []
            variableDeclarations.push(_node)
            objectDeclarationMap.set(object.name, variableDeclarations)
        }
    })

    const declaredNames: string[] = []
    objectPropertyMap.forEach((propertyAccesses, objectName) => {
        if (propertyAccesses.size <= 1) return

        // Rename all variables to their property names
        let insertIndex = body.length
        const destructuringPropertyMap = new Map<string, string>()
        const variableDeclarations = objectDeclarationMap.get(objectName) || []
        variableDeclarations.forEach((variableDeclaration) => {
            if (j.ExpressionStatement.check(variableDeclaration)) {
                const expressionStatement = variableDeclaration as ExpressionStatement
                const expression = expressionStatement.expression as MemberExpression
                const propertyName = (expression.property as Identifier).name

                const newPropertyName = destructuringPropertyMap.get(propertyName)
                    || generateName(propertyName, scope, declaredNames)
                destructuringPropertyMap.set(propertyName, newPropertyName)
                declaredNames.push(newPropertyName)

                const index = body.indexOf(expressionStatement)
                if (index > -1) {
                    insertIndex = Math.min(insertIndex, index)
                    body.splice(index, 1)
                }
                return
            }

            const variableDeclarator = variableDeclaration.declarations[0] as VariableDeclarator
            const variableName = (variableDeclarator.id as Identifier).name
            const propertyName = ((variableDeclarator.init as MemberExpression).property as Identifier).name

            const newPropertyName = destructuringPropertyMap.get(propertyName)
                || generateName(propertyName, scope, declaredNames)
            scope.rename(variableName, newPropertyName)
            destructuringPropertyMap.set(propertyName, newPropertyName)
            declaredNames.push(newPropertyName)

            const index = body.indexOf(variableDeclaration)
            if (index > -1) {
                insertIndex = Math.min(insertIndex, index)
                body.splice(index, 1)
            }
        })

        // Create a new variable declaration with destructuring
        const properties = [...destructuringPropertyMap.entries()]
            .map(([propertyName, newPropertyName]) => {
                const property = j.property(
                    'init',
                    j.identifier(propertyName),
                    j.identifier(newPropertyName),
                )
                property.shorthand = propertyName === newPropertyName
                return property
            })
        const destructuring = j.variableDeclaration('const', [
            j.variableDeclarator(
                j.objectPattern(properties),
                j.identifier(objectName),
            ),
        ])
        body.splice(insertIndex, 0, destructuring)
    })

    objectIndexMap.forEach((indexAccesses, objectName) => {
        if (indexAccesses.filter(nonNull).length <= 1) return

        let insertIndex = body.length
        indexAccesses.forEach((variableName) => {
            const variableDecl = variableDeclarationMap.get(variableName)
            if (!variableDecl) return
            const index = body.indexOf(variableDecl)
            if (index > -1) {
                insertIndex = Math.min(insertIndex, index)
                body.splice(index, 1)
            }
        })

        const arrayPattern = j.arrayPattern(Array.from(indexAccesses, (variableName) => {
            return variableName ? j.identifier(variableName) : null
        }))
        const destructuring = j.variableDeclaration('const', [
            j.variableDeclarator(arrayPattern, j.identifier(objectName)),
        ])
        body.splice(insertIndex, 0, destructuring)
    })
}

export default wrap(transformAST)
