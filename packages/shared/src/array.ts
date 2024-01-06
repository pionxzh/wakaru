export function nonNullable<T>(x: T): x is NonNullable<T> {
    return x != null
}
