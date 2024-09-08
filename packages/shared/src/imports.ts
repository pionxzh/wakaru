export type Source = string
export type Imported = string
export type Local = string

export interface DefaultImport {
    type: 'default'
    name: string
    source: Source
}

export interface NamespaceImport {
    type: 'namespace'
    name: string
    source: Source
}

export interface NamedImport {
    type: 'named'
    name: string
    local: Local
    source: Source
}

export interface BareImport {
    type: 'bare'
    source: Source
}

export type ImportInfo = DefaultImport | NamespaceImport | NamedImport | BareImport
