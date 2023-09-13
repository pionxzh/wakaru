import { toIdentifier } from '@babel/types'

/**
 * Take a module path and generate a valid identifier name
 *
 * For example:
 * - `foo` -> `foo`
 * - `foo-bar` -> `fooBar`
 * - `foo.bar` -> `fooBar`
 * - `@foo/bar` -> `fooBar`
 * - './foo' -> `foo`
 * - './nested/foo' -> `nestedFoo`
 */
export function generateNameFromModulePath(source: string): string {
    const cleanName = source
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
