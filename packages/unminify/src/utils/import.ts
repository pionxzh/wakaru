import { ImportManager } from '@wakaru/ast-utils/imports'
import { findDeclaration } from '@wakaru/ast-utils/scope'
import type { Context, SharedParams } from '@wakaru/ast-utils/wrapAstTransformation'
import type { Scope } from 'ast-types/lib/scope'
import type { JSCodeshift } from 'jscodeshift'

/**
 * Find the local variable name of given module name.
 */
export function findHelperLocals(
    context: Context,
    params: SharedParams,
    moduleName: string,
    moduleEsmName?: string,
): string[] {
    const { j, root, filename } = context

    const importManager = new ImportManager()
    importManager.collectEsModuleImport(j, root)
    importManager.collectCommonJsImport(j, root)
    const imports = importManager.getModuleImports()

    const result: string[] = []

    // find module based on source
    imports
        .filter(imported => imported.source === moduleName || imported.source === moduleEsmName)
        .forEach((imported) => {
            if (imported.type === 'default') {
                result.push(imported.name)
            }
            if (imported.type === 'named') {
                result.push(imported.local)
            }
        })

    // find module based on tags
    const { moduleMapping, moduleMeta } = params
    if (moduleMapping && moduleMeta) {
        const moduleMappingEntries = Object.entries(moduleMapping)

        // helpers in current module
        const currentModuleId = moduleMappingEntries.find(([_, path]) => path === filename)?.[0] || filename
        const currentTags = moduleMeta[currentModuleId]?.tags
        if (currentTags) {
            Object.entries(currentTags).forEach(([local, tags]) => {
                if (tags.includes(moduleName)) {
                    result.push(local)
                }
            })
        }

        // helpers in other modules
        imports.forEach((imported) => {
            const source = moduleMappingEntries.find(([_, path]) => path === imported.source.toString())?.[0] || imported.source
            const targetModule = moduleMeta[source]
            if (!targetModule) return

            if (imported.type === 'named') {
                const targetLocal = targetModule.export[imported.name]
                if (!targetLocal) return

                const targetTags = targetModule.tags[targetLocal]
                if (!targetTags) return

                if (targetTags.includes(moduleName)) {
                    result.push(imported.local)
                }
            }

            if (imported.type === 'default') {
                Object.entries(targetModule.export).forEach(([targetExport, targetLocal]) => {
                    const targetTags = targetModule.tags[targetLocal]
                    if (!targetTags) return

                    if (targetTags.includes(moduleName)) {
                        result.push(`${imported.name}.${targetExport}`)
                    }
                })
            }
        })
    }

    return result
}

export function removeHelperImport(j: JSCodeshift, scope: Scope | null, name: string) {
    if (!scope) return

    const declaration = findDeclaration(scope, name)
    if (!declaration) return

    const importDefaultSpecifier = j(declaration).closest(j.ImportDefaultSpecifier)
    if (importDefaultSpecifier.size() === 1) {
        const importDeclaration = importDefaultSpecifier.closest(j.ImportDeclaration)
        if (importDeclaration.size() === 1) {
            if (importDeclaration.get().node.specifiers.length === 1) {
                importDeclaration.remove()
            }
            else {
                importDefaultSpecifier.remove()
            }
        }
        return
    }

    const importSpecifier = j(declaration).closest(j.ImportSpecifier)
    if (importSpecifier.size() === 1) {
        const importDeclaration = importSpecifier.closest(j.ImportDeclaration)
        if (importDeclaration.size() === 1) {
            if (importDeclaration.get().node.specifiers.length === 1) {
                importDeclaration.remove()
            }
            else {
                importSpecifier.remove()
            }
        }
        return
    }

    const functionDeclaration = j(declaration).closest(j.FunctionDeclaration)
    if (functionDeclaration.size() === 1) {
        functionDeclaration.remove()
        return
    }

    const variableDeclarator = j(declaration).closest(j.VariableDeclarator)
    if (variableDeclarator.size() === 1) {
        const variableDeclaration = variableDeclarator.closest(j.VariableDeclaration)
        if (variableDeclaration.size() === 1) {
            if (variableDeclaration.get().node.declarations.length === 1) {
                variableDeclaration.remove()
            }
            else {
                variableDeclarator.remove()
            }
        }
    }
}
