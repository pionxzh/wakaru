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
export function generateName(input: string, scope: Scope | null = null, existedNames: string[] = []): string {
    const cleanName = input
        .replace(/^@/, '')
        .replace(/^_+/, '')
        .replace(/_+$/, '')
        .replace(/_+/, '_')
        .replace(/^\.+/g, '')
        .replace(/\\+/, '/')
        .replace(/^\/+/, '')
        .replace(/\/+/, '/')
        // leading numbers to _{numbers}
        .replace(/^[-0-9]+/, m => `_${m}`)

    // take last 2 parts of the path
    const candidate = cleanName.split(/\//).slice(-2).join('/')

    const newName = toIdentifier(candidate)
    return getUniqueName(newName, scope, existedNames)
}

function getUniqueName(name: string, scope: Scope | null = null, existedNames: string[] = []): string {
    const isConflict = (n: string) => {
        if (scope && isDeclared(scope, n)) return true
        if (existedNames.includes(n)) return true
        return false
    }
    if (!isConflict(name)) {
        return name
    }

    let i = 1
    while (isConflict(`${name}_${i}`)) {
        i++
    }
    return `${name}_${i}`
}
