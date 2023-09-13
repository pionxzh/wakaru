export function nonNull(x: any): x is NonNullable<typeof x> {
    return x != null
}
