<script setup lang="ts">
// @ts-expect-error - no types
import {
    Dialog,
    DialogPanel,
    DialogTitle,
    TransitionChild,
    TransitionRoot,
} from '@headlessui/vue'
import { unpack } from '@unminify-kit/unpacker'
import { nextTick } from 'vue'
import CodemodWorker from '../codemod.worker?worker'
import Card from '../components/Card.vue'
import CodemirrorEditor from '../components/CodemirrorEditor.vue'
import FileUpload from '../components/FileUpload.vue'
import Separator from '../components/Separator.vue'
import useState from '../composables/shared/useState'
import { useFileIds } from '../composables/useFileIds'
import { useModuleMapping } from '../composables/useModuleMapping'
import { KEY_FILE_PREFIX } from '../const'
import type { TransformedModule } from '../types'

const [source] = useState('')
const [isLoading, setIsLoading] = useState(false)
const [processedCount, setProcessedCount] = useState(0)
const { fileIds } = useFileIds()
const { moduleMapping, setModuleMapping } = useModuleMapping()

function onUpload(file: File) {
    const reader = new FileReader()
    reader.onload = (event) => {
        const value = event.target?.result
        if (typeof value === 'string') {
            run(value)
        }
    }
    reader.readAsText(file)
}

const codemodWorker = new CodemodWorker()
codemodWorker.onmessage = ({ data: module }: MessageEvent<TransformedModule>) => {
    setProcessedCount(count => count + 1)
    localStorage.setItem(`${KEY_FILE_PREFIX}${module.id}`, JSON.stringify(module))

    if (processedCount.value === fileIds.value.length) {
        setIsLoading(false)
    }
}

function getDepName(dep: TransformedModule) {
    return dep.isEntry ? 'entry.js' : `module-${dep.id}.js`
}

function onSubmit() {
    setProcessedCount(0)
    setIsLoading(true)

    nextTick(() => run(source.value))
}

function run(code: string) {
    const result = unpack(code)

    if (!result) {
        // alert('No modules were found in the source code!\nIt might be a bug of unminify-kit.\nPlease report it as an issue.')
        const module = {
            id: 0,
            isEntry: true,
            code,
            transformed: code,
        }
        localStorage.setItem(`${KEY_FILE_PREFIX}${module.id}`, JSON.stringify(module))

        setIsLoading(false)
        return
    }

    const { modules, moduleIdMapping } = result
    const unpackedModules = modules.map<TransformedModule>((module) => {
        const { id, isEntry, code } = module
        return {
            id,
            isEntry,
            code,
            transformed: code,
        }
    })

    fileIds.value = [
        ...unpackedModules.filter(module => module.isEntry).map(module => module.id),
        ...unpackedModules.filter(module => !module.isEntry).map(module => module.id),
    ]

    const newModuleMapping = unpackedModules.reduce((acc, mod) => {
        acc[mod.id] = getDepName(mod)
        return acc
    }, moduleIdMapping)
    // try to preserve the old mapping
    if (Object.keys(newModuleMapping).length !== Object.keys(moduleMapping.value).length) {
        setModuleMapping(newModuleMapping)
    }

    unpackedModules.forEach((module) => {
        // const name = getDepName(module)
        // codemodWorker.postMessage({ name, module })
        localStorage.setItem(`${KEY_FILE_PREFIX}${module.id}`, JSON.stringify(module))
    })
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
                        height: '300px;',
                    }"
                />
            </div>
            <div class="flex justify-center p-4">
                <button
                    class="flex w-fit bg-blue-600 hover:bg-blue-700 text-white font-bold py-2 px-4 rounded transition"
                    @click="onSubmit"
                >
                    Submit
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
