import type { Scope } from 'ast-types/lib/scope'

export function assertExists<T>(
    value: T | null | undefined,
    message: string | Error = 'value does not exist',
): asserts value is T {
    if (value === null || value === undefined) {
        if (message instanceof Error) {
            throw message
        }
        throw new Error(message)
    }
}

export function assertScopeExists(
    scope: Scope | null | undefined,
    message: string | Error = 'scope does not exist in path',
): asserts scope is Scope {
    assertExists(scope, message)
}
