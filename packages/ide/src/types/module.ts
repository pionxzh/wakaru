import type { Module } from '@wakaru/unpacker'

export type TransformedModule = Module & {
    /** The transformed module code */
    transformed: string

    /** Error message if any */
    error?: string
}
