import { ExportManager, ImportManager } from '@unminify-kit/ast-utils'
import { postScanBabelRuntime, scanBabelRuntime } from './babel-runtime'
import type { Module } from '../Module'
import type { JSCodeshift } from 'jscodeshift'

export function scanImports(j: JSCodeshift, module: Module) {
    const root = module.ast
    const importManager = new ImportManager()
    importManager.collectEsModuleImport(j, root)
    importManager.collectCommonJsImport(j, root)
    module.import = importManager.getModuleImports()
}

export function scanExports(j: JSCodeshift, module: Module) {
    const root = module.ast
    const exportManager = new ExportManager()
    exportManager.collectEsModuleExport(j, root)
    exportManager.collectCommonJsExport(j, root)
    module.export = exportManager.toJSON()
}

export function scanRuntime(j: JSCodeshift, module: Module) {
    scanBabelRuntime(j, module)
}

export function postScanRuntime(j: JSCodeshift, modules: Module[]) {
    postScanBabelRuntime(j, modules)
}
