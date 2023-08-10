// Ref: https://github.com/vueuse/vueuse/blob/main/packages/core/useStorage/index.ts

export interface Serializer<T> {
    encode(value: T): string
    decode(raw: string): T
}

const StorageSerializer: Record<'boolean' | 'object' | 'number' | 'any' | 'string', Serializer<any>> = {
    boolean: {
        decode: (v: any) => v === 'true',
        encode: (v: any) => String(v),
    },
    object: {
        decode: (v: any) => JSON.parse(v),
        encode: (v: any) => JSON.stringify(v),
    },
    number: {
        decode: (v: any) => Number.parseFloat(v),
        encode: (v: any) => String(v),
    },
    any: {
        decode: (v: any) => v,
        encode: (v: any) => String(v),
    },
    string: {
        decode: (v: any) => v,
        encode: (v: any) => String(v),
    },
}

export function getDefaultSerializer<T>(defaultValue: T) {
    const type = (defaultValue === null || defaultValue === undefined)
        ? 'any'
        : typeof defaultValue === 'boolean'
            ? 'boolean'
            : typeof defaultValue === 'string'
                ? 'string'
                : typeof defaultValue === 'object'
                    ? 'object'
                    : Array.isArray(defaultValue)
                        ? 'object'
                        : !Number.isNaN(defaultValue)
                                ? 'number'
                                : 'any'
    return StorageSerializer[type]
}
