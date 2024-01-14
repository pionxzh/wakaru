export type MaybeArray<T> = T | T[]

export function arraify<T>(value: T | T[]): T[] {
    if (Array.isArray(value)) {
        return value
    }
    return [value]
}

export function nonNullable<T>(x: T): x is NonNullable<T> {
    return x != null
}
