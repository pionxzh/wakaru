// @ts-expect-error no types
import { isIdentifierName, isKeyword, isStrictReservedWord } from '@babel/helper-validator-identifier'

/**
   * Check if the input `name` is a valid identifier name
   * and isn't a reserved word.
   */
export default function isValidIdentifier(
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
