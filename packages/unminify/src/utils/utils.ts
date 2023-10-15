export function nonNull<T>(x: T): x is NonNullable<T> {
    return x != null
}
