import { ExportManager, ImportManager } from '@unminify-kit/ast-utils'
import { scanBabelRuntime } from './babel-runtime'
import type { Module } from '../Module'
import type { JSCodeshift } from 'jscodeshift'

export function scanModule(j: JSCodeshift, module: Module) {
    scanImports(j, module)
    scanExports(j, module)
    scanRuntime(j, module)
}

function scanImports(j: JSCodeshift, module: Module) {
    const root = module.ast
    const importManager = new ImportManager()
    importManager.collectImportsFromRoot(j, root)
    module.import = importManager.getModuleImports()
}

function scanExports(j: JSCodeshift, module: Module) {
    const root = module.ast
    const exportManager = new ExportManager()
    exportManager.collect(j, root)
    module.export = exportManager.toJSON()
}

function scanRuntime(j: JSCodeshift, module: Module) {
    scanBabelRuntime(j, module)
}
