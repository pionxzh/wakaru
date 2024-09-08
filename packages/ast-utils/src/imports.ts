import { MultiMap } from '@wakaru/ds'
import { isTopLevel } from './isTopLevel'
import type { BareImport, DefaultImport, ImportInfo, Imported, Local, NamedImport, NamespaceImport, Source } from '@wakaru/shared/imports'
import type { ASTNode, ASTPath, CallExpression, Collection, ImportDeclaration, JSCodeshift, StringLiteral, VariableDeclaration, VariableDeclarator } from 'jscodeshift'

export class ImportManager {
    private importSourceOrder = new Set<Source>()
    defaultImports = new MultiMap<Source, Local>()
    namespaceImports = new MultiMap<Source, Local>()
    namedImports = new Map<Source, MultiMap<Imported, Local>>()
    bareImports = new Set<Source>()

    private importDecls: Array<ASTPath<ImportDeclaration>> = []

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
        this.defaultImports.set(source, local)
    }

    removeDefaultImport(source: Source, local: Local) {
        this.defaultImports.remove(source, local)
    }

    addNamespaceImport(source: Source, local: Local) {
        this.namespaceImports.set(source, local)
    }

    addNamedImport(source: Source, imported: Imported, local: Local) {
        if (!this.namedImports.has(source)) {
            this.namedImports.set(source, new MultiMap())
        }
        this.namedImports.get(source)!.set(imported, local)
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

    getNamedImport(local: Local): [Source, MultiMap<Imported, Local>] | undefined {
        return [...this.namedImports.entries()].find(([_source, importedMap]) => [...importedMap.entries()].find(([_imported, locals]) => locals.has(local)))
    }

    getAllLocals(): Local[] {
        return [
            ...[...this.defaultImports.values()].flatMap(locals => [...locals]),
            ...[...this.namespaceImports.values()].flatMap(locals => [...locals]),
            ...[...this.namedImports.values()].flatMap(importedMap => [...importedMap.values()].flatMap(locals => [...locals])),
        ]
    }

    collectEsModuleImport(j: JSCodeshift, root: Collection) {
        root
            .find(j.ImportDeclaration)
            .forEach((path) => {
                const { specifiers, source } = path.node
                if (!j.StringLiteral.check(source)) return

                const sourceValue = source.value
                this.addImportOrder(sourceValue)

                if (!specifiers || specifiers.length === 0) {
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

    collectCommonJsImport(j: JSCodeshift, root: Collection) {
        /**
         * Basic require and require with destructuring
         *
         * @example
         * var foo = require('foo')
         * var { bar } = require('bar')
         * var baz = require('baz').default
         */
        root
            .find(j.VariableDeclaration, {
                declarations: [
                    {
                        type: 'VariableDeclarator',
                        init: (init) => {
                            if (!init) return false
                            if (isRequireCall(j, init)) return true

                            return j.MemberExpression.check(init)
                                && isRequireCall(j, init.object)
                                && j.Identifier.check(init.property)
                                && init.property.name === 'default'
                        },
                    },
                ],
            })
            .forEach((path) => {
                if (!isTopLevel(j, path)) return

                const firstDeclaration = path.node.declarations[0] as VariableDeclarator
                const id = firstDeclaration.id
                const init = j.MemberExpression.check(firstDeclaration.init)
                    ? firstDeclaration.init.object as CallExpression
                    : firstDeclaration.init as CallExpression

                const sourceLiteral = init.arguments[0] as StringLiteral
                const source = sourceLiteral.value

                if (j.Identifier.check(id)) {
                    const local = id.name
                    this.addDefaultImport(source, local)
                    return
                }

                /**
                 * var { bar } = require('bar')
                 */
                if (j.ObjectPattern.check(id)) {
                    id.properties.forEach((property) => {
                        if (j.ObjectProperty.check(property)
                         && j.Identifier.check(property.key)
                         && j.Identifier.check(property.value)
                        ) {
                            const imported = property.key.name
                            const local = property.value.name
                            this.addNamedImport(source, imported, local)
                        }
                    })
                    // eslint-disable-next-line no-useless-return
                    return
                }
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
                const importDeclaration = j.importDeclaration(importSpecifiers, j.stringLiteral(source))
                importStatements.push(importDeclaration)

                if (restDefaultImports.length > 0) {
                    const restImportDeclaration = restDefaultImports.map(info => j.importDeclaration(
                        [j.importDefaultSpecifier(j.identifier(info.name))],
                        j.stringLiteral(source),
                    ))
                    importStatements.push(...restImportDeclaration)
                }
            }

            if (namespaceImports.length > 0) {
                const first = namespaceImports[0]
                const importDeclaration = j.importDeclaration(
                    [j.importNamespaceSpecifier(j.identifier(first.name!))],
                    j.stringLiteral(source),
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
                const importDeclaration = j.importDeclaration([], j.stringLiteral(source))
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

function isRequireCall(j: JSCodeshift, node: ASTNode) {
    return j.match(node, {
        type: 'CallExpression',
        callee: {
            type: 'Identifier',
            name: 'require',
        },
        // @ts-expect-error
        arguments: (args) => {
            if (args.length !== 1) return false
            return j.StringLiteral.check(args[0])
                || j.NumericLiteral.check(args[0])
        },
    })
}
