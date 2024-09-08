import {
    delMany as delIdbMany,
    get as getIdb,
    getMany as getIdbMany,
    keys as keysIdb,
    set as setIdb,
    setMany as setIdbMany } from 'idb-keyval'
import { atom, getDefaultStore } from 'jotai/vanilla'
import { KEY_FILE_PREFIX, KEY_MODULE_MAPPING, KEY_MODULE_META } from '../const'
import { unminify } from '../worker'
import { enabledRuleIdsAtom, prettifyRules } from './rule'
import type { ImportInfo } from '@wakaru/shared/imports'
import type { ModuleMapping, ModuleMeta } from '@wakaru/shared/types'

export type ModuleId = number | string

export interface CodeModule {
    id: ModuleId

    /** Whether the module is the entry module */
    isEntry: boolean

    code: string

    transformed: string

    /** A list of import meta */
    import: ImportInfo[]

    /** A map of exported name to local identifier */
    export: Record<string, string>

    /**
     * A map of top-level local identifier to a list of tags.
     * A tag represents a special meaning of the identifier.
     * For example, a function can be marked as a runtime
     * function, and be properly transformed by corresponding
     * rules.
     */
    tags: Record<string, string[]>
}

export type CodeModuleWithName = CodeModule & { name: string }

export function getDefaultCodeModule(moduleId: ModuleId = -1): CodeModule {
    return {
        id: moduleId,
        isEntry: false,
        code: '',
        transformed: '',
        import: [],
        export: {},
        tags: {},
    }
}

export function getModuleDefaultName(module: CodeModule) {
    return module.isEntry ? `entry-${module.id}.js` : `module-${module.id}.js`
}

const _moduleMappingAtom = atom<ModuleMapping>({})
export const moduleMappingAtom = atom(
    get => get(_moduleMappingAtom),
    (_get, set, newMapping: ModuleMapping) => {
        set(_moduleMappingAtom, newMapping)
        setIdb(KEY_MODULE_MAPPING, newMapping)
    },
)

const _modulesAtom = atom<CodeModule[]>([])
export const modulesAtom = atom(
    (get) => {
        const modules = get(_modulesAtom)
        const moduleMapping = get(moduleMappingAtom)

        const moduleWithNames = modules.map((mod) => {
            const mappedName = moduleMapping[mod.id]
            if (mappedName) return { ...mod, name: mappedName }

            return { ...mod, name: getModuleDefaultName(mod) }
        })

        return [
            ...moduleWithNames.filter(mod => mod.isEntry).sort((a, b) => +a.id - +b.id),
            ...moduleWithNames.filter(mod => !mod.isEntry).sort((a, b) => +a.id - +b.id),
        ]
    },
    (get, set, adds: CodeModule[], updates: CodeModule[], deletes: ModuleId[], skipSync = false) => {
        const modules = get(_modulesAtom)
        const newModules = [...modules, ...adds].filter(mod => !deletes.includes(mod.id))
        updates.forEach((mod) => {
            const idx = newModules.findIndex(m => m.id === mod.id)
            if (idx !== -1) newModules[idx] = mod
        })
        set(_modulesAtom, newModules)

        if (adds.length > 0) {
            const moduleMapping = get(moduleMappingAtom)
            const newModuleMapping = { ...moduleMapping }

            let changed = false
            adds.forEach((mod) => {
                if (moduleMapping[mod.id]) return
                changed = true
                newModuleMapping[mod.id] = getModuleDefaultName(mod)
            })
            if (changed) {
                set(moduleMappingAtom, newModuleMapping)
            }
        }

        if (!skipSync) {
            setIdbMany([...adds, ...updates].map(mod => [`${KEY_FILE_PREFIX}${mod.id}`, mod]))
            delIdbMany(deletes.map(modId => `${KEY_FILE_PREFIX}${modId}`))
        }
    },
)

export function getModuleAtom(moduleId: ModuleId) {
    moduleId = moduleId.toString()
    return atom(
        (get) => {
            const modules = get(modulesAtom)
            return modules.find(mod => mod.id.toString() === moduleId) || {
                name: `module-${moduleId}.js`,
                ...getDefaultCodeModule(moduleId),
            }
        },
        (get, set, updateValue: Partial<Exclude<CodeModule, 'id'>>) => {
            const modules = get(modulesAtom)
            const moduleIdx = modules.findIndex(mod => mod.id.toString() === moduleId)
            if (moduleIdx === -1) return

            const module = modules[moduleIdx]
            const updatedModule = { ...module, ...updateValue }
            set(modulesAtom, [], [updatedModule], [])
        },
    )
}

type ModuleAtom = ReturnType<typeof getModuleAtom>

/**
 * Module Meta is a computed result of all modules.
 *
 * This atom is used to override the computed result. Used by shared url.
 */
const _moduleMetaOverrideAtom = atom<ModuleMeta | null>(null)
export const moduleMetaOverrideAtom = atom(
    get => get(_moduleMetaOverrideAtom),
    (_get, set, newMeta: ModuleMeta | null) => {
        set(_moduleMetaOverrideAtom, newMeta)
        setIdb(KEY_MODULE_META, newMeta)
    },
)

export const moduleMetaAtom = atom<ModuleMeta>((get) => {
    const moduleMetaOverride = get(moduleMetaOverrideAtom)
    if (moduleMetaOverride) return moduleMetaOverride

    const modules = get(modulesAtom)
    const moduleMeta = modules.reduce((acc, mod) => {
        acc[mod.id] = {
            import: mod.import,
            export: mod.export,
            tags: mod.tags,
        }
        return acc
    }, {} as ModuleMeta)

    return moduleMeta
})

export const prettifyAllModulesAtom = atom(null, async (get, set) => {
    const modules = get(modulesAtom)

    await Promise.all(modules.map(async (mod) => {
        const result = await unminify({
            name: mod.name,
            module: mod,
            transformationRuleIds: prettifyRules,
            moduleMeta: {},
            moduleMapping: {},
        })

        if (mod.code !== result.transformed) {
            set(modulesAtom, [], [{ ...mod, code: result.transformed }], [])
        }
    }))
})

// export const prettifyModuleAtom = atom(null, async (get, set, moduleAtom: ModuleAtom) => {
//     const module = get(moduleAtom)
//     const result = await unminify({
//         name: module.name,
//         module,
//         transformationRuleIds: prettifyRules,
//         moduleMeta: {},
//         moduleMapping: {},
//     })
//     if (module.code === result.transformed) return

//     set(moduleAtom, { code: result.transformed })
// })

export const unminifyModuleAtom = atom(null, async (get, set, moduleAtom: ModuleAtom) => {
    const module = get(moduleAtom)
    const moduleMeta = get(moduleMetaAtom)
    const moduleMapping = get(moduleMappingAtom)
    const transformationRuleIds = get(enabledRuleIdsAtom)

    const result = await unminify({
        name: module.name,
        module,
        transformationRuleIds,
        moduleMeta,
        moduleMapping,
    })
    if (module.code === result.transformed) return

    set(moduleAtom, { transformed: result.transformed })
})

export const resetModulesAtom = atom(null, (get, set) => {
    set(moduleMetaOverrideAtom, null)
    set(moduleMappingAtom, {})

    const modules = get(modulesAtom)
    set(modulesAtom, [], [], modules.map(mod => mod.id))
})

export async function prepare() {
    const keys = await keysIdb()
    const moduleKeys = keys.filter(key => typeof key === 'string' && key.startsWith(KEY_FILE_PREFIX))
    if (moduleKeys.length > 0) {
        const moduleMapping = await getIdb(KEY_MODULE_MAPPING)
        if (moduleMapping) {
            getDefaultStore().set(moduleMappingAtom, moduleMapping)
        }

        const moduleMeta = await getIdb(KEY_MODULE_META)
        if (moduleMeta) {
            getDefaultStore().set(moduleMetaOverrideAtom, moduleMeta)
        }

        const modules = await getIdbMany(moduleKeys)
        getDefaultStore().set(modulesAtom, modules, [], [], true)
    }
}
