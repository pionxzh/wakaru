import type { NodePath } from 'ast-types/lib/node-path'
import type { Collection, ImportDeclaration, JSCodeshift, VariableDeclaration } from 'jscodeshift'

type Source = string
type Imported = string
type Local = string

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

export class ImportManager {
    private importSourceOrder = new Set<Source>()
    defaultImports = new Map<Source, Set<Local>>()
    namespaceImports = new Map<Source, Set<Local>>()
    namedImports = new Map<Source, Map<Imported, Set<Local>>>()
    bareImports = new Set<Source>()

    private importDecls: Array<NodePath<ImportDeclaration>> = []

    get importMap() {
        /**
         * Bare import can be omitted if there are other imports from the same source
         */
        const bareImports = [...this.bareImports.values()]
            .filter((source) => {
                return !this.defaultImports.has(source)
                    && !this.namespaceImports.has(source)
                    && !this.namedImports.has(source)
            })

        const importMap = [
            ...[...this.defaultImports.entries()].flatMap(([source, locals]) => [...locals].map(local => ({ type: 'default', name: local, source } as const))),
            ...[...this.namespaceImports.entries()].flatMap(([source, locals]) => [...locals].map(local => ({ type: 'namespace', name: local, source } as const))),
            ...[...this.namedImports.entries()].flatMap(([source, importedMap]) => [...importedMap.entries()].flatMap(([imported, locals]) => [...locals].map(local => ({ type: 'named', name: imported, local, source } as const)))),
            ...bareImports.map(source => ({ type: 'bare', source } as const)),
        ].reduce((map, info) => {
            if (!map[info.source]) {
                map[info.source] = []
            }

            map[info.source].push(info)

            return map
        }, {} as Record<Source, ImportInfo[]>)

        const importSourceOrder = [...this.importSourceOrder.values()]
        return Object.entries(importMap).sort(([a], [b]) => {
            const aIndex = importSourceOrder.indexOf(a)
            const bIndex = importSourceOrder.indexOf(b)
            return bIndex - aIndex
        })
    }

    getModuleImports() {
        const defaultImports = [...this.defaultImports.entries()].flatMap(([source, locals]) => [...locals].map(local => ({ type: 'default', name: local, source } as const)))
        const namespaceImports = [...this.namespaceImports.entries()].flatMap(([source, locals]) => [...locals].map(local => ({ type: 'namespace', name: local, source } as const)))
        const namedImports = [...this.namedImports.entries()].flatMap(([source, importedMap]) => [...importedMap.entries()].flatMap(([imported, locals]) => [...locals].map(local => ({ type: 'named', name: imported, local, source } as const))))
        const bareImports = [...this.bareImports.values()].map(source => ({ type: 'bare', source } as const))
        return [...defaultImports, ...namespaceImports, ...namedImports, ...bareImports]
    }

    static fromModuleImports(moduleImports: ImportInfo[]) {
        const collector = new ImportManager()
        moduleImports.forEach((info) => {
            switch (info.type) {
                case 'default':
                    collector.addDefaultImport(info.source, info.name)
                    break
                case 'namespace':
                    collector.addNamespaceImport(info.source, info.name)
                    break
                case 'named':
                    collector.addNamedImport(info.source, info.name, info.local)
                    break
                case 'bare':
                    collector.addBareImport(info.source)
                    break
            }
        })
        return collector
    }

    addImportOrder(source: Source) {
        this.importSourceOrder.add(source)
    }

    addDefaultImport(source: Source, local: Local) {
        if (!this.defaultImports.has(source)) this.defaultImports.set(source, new Set())
        this.defaultImports.get(source)!.add(local)
    }

    addNamespaceImport(source: Source, local: Local) {
        if (!this.namespaceImports.has(source)) {
            this.namespaceImports.set(source, new Set())
        }
        this.namespaceImports.get(source)!.add(local)
    }

    addNamedImport(source: Source, imported: Imported, local: Local) {
        if (!this.namedImports.has(source)) {
            this.namedImports.set(source, new Map())
        }
        if (!this.namedImports.get(source)!.has(imported)) {
            this.namedImports.get(source)!.set(imported, new Set())
        }
        this.namedImports.get(source)!.get(imported)!.add(local)
    }

    addBareImport(source: Source) {
        this.bareImports.add(source)
    }

    getImport(local: Local) {
        return this.getDefaultImport(local)
            || this.getNamespaceImport(local)
            || this.getNamedImport(local)
    }

    getDefaultImport(local: Local): [Source, Set<Local>] | undefined {
        return [...this.defaultImports.entries()].find(([_source, locals]) => locals.has(local))
    }

    getNamespaceImport(local: Local): [Source, Set<Local>] | undefined {
        return [...this.namespaceImports.entries()].find(([_source, locals]) => locals.has(local))
    }

    getNamedImport(local: Local): [Source, Map<Imported, Set<Local>>] | undefined {
        return [...this.namedImports.entries()].find(([_source, importedMap]) => [...importedMap.entries()].find(([_imported, locals]) => locals.has(local)))
    }

    getAllLocals(): Local[] {
        return [
            ...[...this.defaultImports.values()].flatMap(locals => [...locals]),
            ...[...this.namespaceImports.values()].flatMap(locals => [...locals]),
            ...[...this.namedImports.values()].flatMap(importedMap => [...importedMap.values()].flatMap(locals => [...locals])),
        ]
    }

    collectImportsFromRoot(j: JSCodeshift, root: Collection) {
        root
            .find(j.ImportDeclaration)
            .forEach((path) => {
                const { specifiers, source } = path.node
                if (!j.Literal.check(source) || typeof source.value !== 'string') return

                const sourceValue = source.value
                this.addImportOrder(sourceValue)

                if (!specifiers) {
                    this.addBareImport(sourceValue)
                    this.importDecls.push(path)
                    return
                }

                specifiers.forEach((specifier) => {
                    if (j.ImportDefaultSpecifier.check(specifier)
                        && j.Identifier.check(specifier.local)) {
                        const local = specifier.local.name
                        this.addDefaultImport(sourceValue, local)
                    }

                    if (j.ImportSpecifier.check(specifier)
                        && j.Identifier.check(specifier.imported)
                        && j.Identifier.check(specifier.local)) {
                        this.addNamedImport(
                            sourceValue,
                            specifier.imported.name,
                            specifier.local.name,
                        )
                    }

                    if (j.ImportNamespaceSpecifier.check(specifier)
                        && j.Identifier.check(specifier.local)) {
                        this.addNamespaceImport(
                            sourceValue,
                            specifier.local.name,
                        )
                    }
                })
                this.importDecls.push(path)
            })
    }

    /**
     * Remove all collected import declarations
     */
    private removeCollectedImportDeclarations() {
        this.importDecls.forEach((path) => {
            path.prune()
        })
    }

    /**
     * Remove all collected import declarations and insert
     * new import declarations to the top of the file.
     */
    applyImportToRoot(j: JSCodeshift, root: Collection) {
        this.removeCollectedImportDeclarations()

        this.importMap.forEach(([source, infos]) => {
            const importStatements: ImportDeclaration[] = []
            const variableDeclarations: VariableDeclaration[] = []

            const namedImports = infos.filter(info => info.type === 'named') as NamedImport[]
            const defaultImports = infos.filter(info => info.type === 'default') as DefaultImport[]
            const namespaceImports = infos.filter(info => info.type === 'namespace') as NamespaceImport[]
            const bareImports = infos.filter(info => info.type === 'bare') as BareImport[]

            if (namedImports.length > 0 || defaultImports.length > 0) {
                const [firstDefaultImport, ...restDefaultImports] = defaultImports

                const importSpecifiers = [
                    ...(firstDefaultImport ? [j.importDefaultSpecifier(j.identifier(firstDefaultImport.name))] : []),
                    ...namedImports.map(info => j.importSpecifier(j.identifier(info.name), j.identifier(info.local))),
                ]
                const importDeclaration = j.importDeclaration(importSpecifiers, j.literal(source))
                importStatements.push(importDeclaration)

                if (restDefaultImports.length > 0) {
                    const restImportDeclaration = restDefaultImports.map(info => j.importDeclaration(
                        [j.importDefaultSpecifier(j.identifier(info.name))],
                        j.literal(source),
                    ))
                    importStatements.push(...restImportDeclaration)
                }
            }

            if (namespaceImports.length > 0) {
                const first = namespaceImports[0]
                const importDeclaration = j.importDeclaration(
                    [j.importNamespaceSpecifier(j.identifier(first.name!))],
                    j.literal(source),
                )
                importStatements.push(importDeclaration)

                /**
                 * Other namespace imports should be aliased to the first one
                 */
                if (namespaceImports.length > 1) {
                    const variableDeclaration = j.variableDeclaration(
                        'const',
                        namespaceImports.slice(1).map(info => j.variableDeclarator(j.identifier(info.name!), j.identifier(first.name!))),
                    )
                    variableDeclarations.push(variableDeclaration)
                }
            }

            /**
             * Bare import is not needed if there are other imports
             */
            if (bareImports.length > 0) {
                const importDeclaration = j.importDeclaration([], j.literal(source))
                importStatements.push(importDeclaration)
            }

            if (variableDeclarations.length > 0) {
                root.find(j.Program).get('body', 0).insertBefore(...variableDeclarations)
            }

            // insert to the top of the file
            root.find(j.Program).get('body', 0).insertBefore(...importStatements)
        })
    }

    reset() {
        this.importSourceOrder.clear()
        this.defaultImports.clear()
        this.namespaceImports.clear()
        this.namedImports.clear()
        this.bareImports.clear()
    }
}
