<script setup lang="ts">
import {
    Dialog,
    DialogPanel,
    DialogTitle,
    TransitionChild,
    TransitionRoot,
} from '@headlessui/vue'
import { useSetAtom } from 'jotai-vue'
import { useRoute, useRouter } from 'vue-router'
import { disabledRuleIdsAtom } from '../atoms/rule'
import Card from '../components/Card.vue'
import CodemirrorEditor from '../components/CodemirrorEditor.vue'
import FileUpload from '../components/FileUpload.vue'
import Separator from '../components/Separator.vue'
import useState from '../composables/shared/useState'
import { decodeHash } from '../composables/url'
import { useFileIds } from '../composables/useFileIds'
import { useModuleMapping } from '../composables/useModuleMapping'
import { useModuleMeta } from '../composables/useModuleMeta'
import { KEY_FILE_PREFIX } from '../const'
import { unminify, unpack } from '../worker'
import type { TransformedModule } from '../types'
import type { ModuleMapping, ModuleMeta } from '@wakaru/ast-utils/types'

const router = useRouter()
const route = useRoute()

const [source] = useState('')
const [isLoading, setIsLoading] = useState(false)
const [processedCount, setProcessedCount] = useState(0)

const setDisabledRuleIds = useSetAtom(disabledRuleIdsAtom)
const { fileIds, setFileIds } = useFileIds()
const { moduleMeta, setModuleMeta } = useModuleMeta()
const { moduleMapping, setModuleMapping } = useModuleMapping()

let existingMeta: ModuleMeta
let existingMapping: ModuleMapping | undefined

onLoad()

function onLoad() {
    if (typeof route.hash === 'string' && route.hash.startsWith('#')) {
        const hash = route.hash.slice(1)
        const { code, rules, mapping, meta } = decodeHash(hash)
        if (rules) setDisabledRuleIds(rules)
        if (mapping) existingMapping = mapping
        if (meta) existingMeta = meta
        if (code) startUnpack(code)
    }
}

function onUpload(file: File) {
    const reader = new FileReader()
    reader.onload = (event) => {
        const scriptContent = event.target?.result
        if (!scriptContent || typeof scriptContent !== 'string') return

        startUnpack(scriptContent)
    }
    reader.readAsText(file)
}

function onSubmit() {
    if (!source.value) return

    startUnpack(source.value)
}

function reset() {
    // Clear all old files
    Object.keys(moduleMapping.value).forEach(key => localStorage.removeItem(`${KEY_FILE_PREFIX}${key}`))

    setFileIds([])
    setModuleMeta({})
}

async function startUnpack(input: string) {
    setProcessedCount(0)
    setIsLoading(true)

    reset()

    try {
        const { modules, moduleIdMapping } = await unpack(input)
        const unpackedModules = modules.map<TransformedModule>((module) => {
            const { id, isEntry, code, tags } = module
            return {
                id,
                isEntry,
                code,
                transformed: code,
                import: module.import,
                export: module.export,
                tags,
            }
        })

        setFileIds([
            ...unpackedModules.filter(module => module.isEntry).map(module => module.id).sort((a, b) => +a - +b),
            ...unpackedModules.filter(module => !module.isEntry).map(module => module.id).sort((a, b) => +a - +b),
        ])

        if (existingMeta) {
            setModuleMeta(existingMeta)
        }
        else {
            setModuleMeta(
                unpackedModules.reduce((acc, mod) => {
                    acc[mod.id] = {
                        import: mod.import,
                        export: mod.export,
                        tags: mod.tags,
                    }
                    return acc
                }, moduleMeta.value),
            )
        }

        const newModuleMapping = unpackedModules.reduce((acc, mod) => {
            acc[mod.id] = getDepName(mod)
            return acc
        }, moduleIdMapping)

        if (existingMapping) {
            setModuleMapping(existingMapping)
        }
        // try to preserve the old mapping if possible
        else if (Object.keys(newModuleMapping).length !== Object.keys(moduleMapping.value).length) {
            setModuleMapping(newModuleMapping)
        }

        const rules = [
            'un-sequence-expression1',
            'un-variable-merging',
            'prettier',
        ]
        const mapping = moduleMapping.value

        for (const module of unpackedModules) {
            const moduleName = mapping[module.id]
            // Do a pre-formatting to improve the readability of the code
            const result = await unminify({
                name: moduleName,
                module,
                transformationRuleIds: rules,
                moduleMeta: moduleMeta.value,
                moduleMapping: mapping,
            })
            module.code = result.transformed
            module.transformed = result.transformed

            setProcessedCount(count => count + 1)

            localStorage.setItem(`${KEY_FILE_PREFIX}${module.id}`, JSON.stringify(module))
        }
    }
    catch (error) {
        console.error(error)
        // eslint-disable-next-line no-alert
        alert('Failed to unpack the code. Please check the console and report the issue.')
    }

    setIsLoading(false)

    const firstFileId = fileIds.value[0]
    if (firstFileId !== undefined) {
        router.push({ name: 'file', params: { id: firstFileId } })
    }
}

function getDepName(dep: TransformedModule) {
    return dep.isEntry ? 'entry.js' : `module-${dep.id}.js`
}
</script>

<template>
    <Card
        title="Upload"
        description="You can either upload a source file or paste the code into the editor below."
    >
        <div class="flex flex-col w-full">
            <FileUpload accept=".txt,.js" @upload="onUpload" />

            <Separator>OR</Separator>

            <div class="flex-1">
                <CodemirrorEditor
                    v-model="source"
                    autofocus
                    :style="{
                        height: 'calc(100vh - 26rem)',
                    }"
                />
            </div>
            <div class="flex justify-center p-4">
                <button
                    class="flex w-fit bg-blue-600 hover:bg-blue-700 text-white font-bold py-2 px-4 rounded transition"
                    @click="onSubmit"
                >
                    Start
                </button>
            </div>
        </div>
    </Card>

    <TransitionRoot appear :show="isLoading" as="template">
        <Dialog as="div" class="relative z-10">
            <TransitionChild
                as="template"
                enter="duration-300 ease-out"
                enter-from="opacity-0"
                enter-to="opacity-100"
                leave="duration-200 ease-in"
                leave-from="opacity-100"
                leave-to="opacity-0"
            >
                <div class="fixed inset-0 bg-black bg-opacity-25" />
            </TransitionChild>

            <div class="fixed inset-0 overflow-y-auto">
                <div
                    class="flex min-h-full items-center justify-center p-4 text-center"
                >
                    <TransitionChild
                        as="template"
                        enter="duration-300 ease-out"
                        enter-from="opacity-0 scale-95"
                        enter-to="opacity-100 scale-100"
                        leave="duration-200 ease-in"
                        leave-from="opacity-100 scale-100"
                        leave-to="opacity-0 scale-95"
                    >
                        <DialogPanel
                            class="w-full max-w-md transform overflow-hidden rounded-2xl bg-white p-6 text-left align-middle shadow-xl transition-all"
                        >
                            <DialogTitle
                                as="h3"
                                class="text-lg font-medium leading-6 text-gray-900"
                            >
                                Processing...
                            </DialogTitle>
                            <div class="mt-2">
                                <p v-if="fileIds.length > 0" class="text-sm text-gray-500">
                                    Processed ({{ processedCount }}/{{ fileIds.length }}) files
                                </p>
                            </div>
                        </DialogPanel>
                    </TransitionChild>
                </div>
            </div>
        </Dialog>
    </TransitionRoot>
</template>
