import { isValidIdentifier } from './identifier'
import type { JSCodeshift, ObjectProperty } from 'jscodeshift'

export function createObjectProperty(j: JSCodeshift, key: string | ObjectProperty['key'], value: ObjectProperty['value']) {
    const normalizedKey = j.Identifier.check(key)
        ? isValidIdentifier(key.name) ? key : j.stringLiteral(key.name)
        : typeof key === 'string'
            ? isValidIdentifier(key) ? j.identifier(key) : j.stringLiteral(key)
            : key
    const property = j.objectProperty(
        normalizedKey,
        value,
    )

    return property
}
