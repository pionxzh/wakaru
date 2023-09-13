// @ts-expect-error no types
import { isIdentifierName, isKeyword, isStrictReservedWord } from '@babel/helper-validator-identifier'
import { toIdentifier } from '@babel/types'
import { isDeclared } from './scope'
import type { Scope } from 'ast-types/lib/scope'

/**
 * Copied from https://github.com/babel/babel/blob/6e04ebdb33da39d3ad5b6bbda8c42ff3daa8dab2/packages/babel-types/src/validators/isValidIdentifier.ts#L11
 * Check if the input `name` is a valid identifier name
 * and isn't a reserved word.
 */
export function isValidIdentifier(
    name: string,
    reserved = true,
): boolean {
    if (typeof name !== 'string') return false

    if (reserved) {
        // "await" is invalid in module, valid in script; better be safe (see #4952)
        if (isKeyword(name) || isStrictReservedWord(name, true)) {
            return false
        }
    }

    return isIdentifierName(name)
}

/**
 * Generate a valid identifier name
 *
 * An optional `scope` can be provided to ensure the generated name is unique.
 * It will append a `$` and a number to the name if it's not unique.
 *
 * For example:
 * - `foo` -> `foo`
 * - `foo-bar` -> `fooBar`
 * - `foo.bar` -> `fooBar`
 * - `@foo/bar` -> `fooBar`
 * - './foo' -> `foo`
 * - './nested/foo' -> `nestedFoo`
 */
export function generateName(input: string, scope?: Scope): string {
    const cleanName = input
        .replace(/^@/, '')
        .replace(/^_+/, '')
        .replace(/_+$/, '')
        .replace(/_+/, '_')
        .replace(/^\.+/g, '')
        .replace(/\\+/, '/')
        .replace(/^\/+/, '')
        .replace(/\/+/, '/')

    // take last 2 parts of the path
    const candidate = cleanName.split(/\//).slice(-2).join('/')

    const newName = toIdentifier(candidate)
    return scope ? getUniqueName(scope, newName) : newName
}

function getUniqueName(scope: Scope, name: string): string {
    if (!isDeclared(scope, name)) return name

    let i = 0
    while (scope.declares(`${name}$${i}`)) {
        i++
    }
    return `${name}$${i}`
}
