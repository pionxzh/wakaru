import type { Scope } from 'ast-types/lib/scope'

export function isDeclared(scope: Scope, name: string) {
    while (scope) {
        if (scope.declares(name)) return true
        scope = scope.parent
    }

    return false
}
