// @ts-expect-error no types
import { isIdentifierName, isKeyword, isStrictReservedWord } from '@babel/helper-validator-identifier'

/**
 * Copied from https://github.com/babel/babel/blob/6e04ebdb33da39d3ad5b6bbda8c42ff3daa8dab2/packages/babel-types/src/validators/isValidIdentifier.ts#L11
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
