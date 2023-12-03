import { ExportManager } from '@wakaru/ast-utils/exports'
import { ImportManager } from '@wakaru/ast-utils/imports'
import { postScanBabelRuntime, scanBabelRuntime } from './babel-runtime'
import type { Module } from '../Module'
import type { Collection, JSCodeshift } from 'jscodeshift'

export function scanImports(j: JSCodeshift, module: Module & { root: Collection }) {
    const importManager = new ImportManager()
    importManager.collectEsModuleImport(j, module.root)
    importManager.collectCommonJsImport(j, module.root)
    module.import = importManager.getModuleImports()
}

export function scanExports(j: JSCodeshift, module: Module & { root: Collection }) {
    const exportManager = new ExportManager()
    exportManager.collectEsModuleExport(j, module.root)
    exportManager.collectCommonJsExport(j, module.root)
    module.export = exportManager.toJSON()
}

export function scanRuntime(j: JSCodeshift, module: Module & { root: Collection }) {
    scanBabelRuntime(j, module)
}

export function postScanRuntime(j: JSCodeshift, modules: (Module & { root: Collection })[]) {
    postScanBabelRuntime(j, modules)
}
