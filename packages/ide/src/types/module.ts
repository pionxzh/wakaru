import type { Module } from '@wakaru/unpacker'

export type TransformedModule = Omit<Module, 'ast' | 'code'> & {
    /** The module's code */
    code: string

    /** The transformed module code */
    transformed: string

    /** Error message if any */
    error?: string
}
