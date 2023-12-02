import { strFromU8, strToU8, unzlibSync, zlibSync } from 'fflate'
import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'

// ref: https://github.com/sxzz/ast-explorer/blob/c107e71da6fb1582349cc64607a46aaa3c2280c9/composables/url.ts
function utoa(data: string): string {
    const buffer = strToU8(data)
    const zipped = zlibSync(buffer, { level: 9 })
    const binary = strFromU8(zipped, true)
    return btoa(binary)
}

function atou(base64: string): string {
    const binary = atob(base64)

    // zlib header (x78), level 9 (xDA)
    if (binary.startsWith('\u0078\u00DA')) {
        const buffer = strToU8(binary, true)
        const unzipped = unzlibSync(buffer)
        return strFromU8(unzipped)
    }

    // old unicode hacks for backward compatibility
    // https://base64.guru/developers/javascript/examples/unicode-strings
    return decodeURIComponent(escape(binary))
}

interface DecodedOptions {
    code?: string
    rules?: string[]
    mapping?: ModuleMapping
    meta?: ModuleMeta
}

export function encodeOption(data: DecodedOptions): string {
    return utoa(JSON.stringify(data))
}

export function decodeHash(base64: string): DecodedOptions {
    try {
        const result = JSON.parse(atou(base64))
        return result
    }
    catch (error) {
        return {}
    }
}
