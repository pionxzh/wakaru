export type FileIdList = Array<number | string>

export interface TransformedModule {
    /** The module id */
    id: string | number

    /** Whether the module is the entry module */
    isEntry: boolean

    /** The module code */
    code: string

    /** The transformed module code */
    transformed: string

    /** Error message if any */
    error?: string
}
