// @ts-expect-error no types
import { isIdentifierName, isKeyword, isStrictReservedWord } from '@babel/helper-validator-identifier'
import { toIdentifier } from '@babel/types'

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
 * For example:
 * - `foo` -> `foo`
 * - `foo-bar` -> `fooBar`
 * - `foo.bar` -> `fooBar`
 * - `@foo/bar` -> `fooBar`
 * - './foo' -> `foo`
 * - './nested/foo' -> `nestedFoo`
 */
export function generateName(input: string): string {
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

    return toIdentifier(candidate)
}
