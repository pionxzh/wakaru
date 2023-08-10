<script setup lang="ts">
import { Dialog, DialogPanel, TransitionChild, TransitionRoot } from '@headlessui/vue'
import { transformationMap } from '@unminify-kit/unminify'
import { computed, watch } from 'vue'
import { useRoute } from 'vue-router'
import Card from '../components/Card.vue'
import CodemirrorEditor from '../components/CodemirrorEditor.vue'
import { useLocalStorage } from '../composables/shared/useLocalStorage'
import useState from '../composables/shared/useState'
import { useCodemod } from '../composables/useCodemod'
import { useModule } from '../composables/useModule'
import { useModuleMapping } from '../composables/useModuleMapping'

const { params: { id } } = useRoute()
const { module, setModule } = useModule(id as string)
const { moduleMapping } = useModuleMapping()

const [openSideBar, setOpenSideBar] = useState(false)
const transformations = Object.keys(transformationMap)
const [enabledTransformations] = useLocalStorage('app:transformations', transformations)
function toggleTransformation(transformation: string) {
    if (enabledTransformations.value.includes(transformation)) {
        enabledTransformations.value = enabledTransformations.value.filter(t => t !== transformation && transformations.includes(t))
    }
    else {
        enabledTransformations.value = [...enabledTransformations.value, transformation]
            .filter(t => transformations.includes(t))
            .sort((a, b) => transformations.indexOf(a) - transformations.indexOf(b))
    }
}

const { transform } = useCodemod()
const moduleName = computed(() => moduleMapping.value[module.value.id])

watch([enabledTransformations, () => module.value.code], async () => {
    const result = await transform(moduleName.value, module.value, enabledTransformations.value, moduleMapping.value)
    setModule({ ...module.value, transformed: result.transformed })
}, { immediate: true })
</script>

<template>
    <div class="flex flex-row h-full">
        <Card title="Source">
            <div class="w-full">
                <CodemirrorEditor
                    :model-value="module.code"
                    :style="{
                        height: '500px;',
                    }"
                    @update:model-value="setModule({ ...module, code: $event })"
                />
            </div>
        </Card>
        <Card title="Transformed">
            <div class="w-full">
                <button class="flex items-center justify-center px-4 py-2 border-none text-sm font-medium rounded-md text-white bg-blue-600 hover:bg-blue-700 focus:outline-none" @click="setOpenSideBar(true)">
                    Edit Rules
                </button>
                <CodemirrorEditor
                    :model-value="module?.transformed"
                    :style="{
                        height: '500px;',
                    }"
                />
            </div>
        </Card>
    </div>
    <TransitionRoot as="template" :show="openSideBar">
        <Dialog as="div" class="relative z-10" @close="setOpenSideBar(false)">
            <TransitionChild as="template" enter="ease-in-out duration-500" enter-from="opacity-0" enter-to="opacity-100" leave="ease-in-out duration-500" leave-from="opacity-100" leave-to="opacity-0">
                <div class="fixed inset-0 bg-gray-500 dark:bg-black bg-opacity-75 dark:bg-opacity-50 transition-opacity" />
            </TransitionChild>

            <div class="fixed inset-0 overflow-hidden">
                <div class="absolute inset-0 overflow-hidden">
                    <div class="pointer-events-none fixed inset-y-0 right-0 flex max-w-full pl-10">
                        <TransitionChild as="template" enter="transform transition ease-in-out duration-500 sm:duration-700" enter-from="translate-x-full" enter-to="translate-x-0" leave="transform transition ease-in-out duration-500 sm:duration-700" leave-from="translate-x-0" leave-to="translate-x-full">
                            <DialogPanel class="pointer-events-auto relative w-screen max-w-md">
                                <!-- <TransitionChild as="template" enter="ease-in-out duration-500" enter-from="opacity-0" enter-to="opacity-100" leave="ease-in-out duration-500" leave-from="opacity-100" leave-to="opacity-0">
                                    <div
                                        class="absolute flex justify-center items-center top-0 left-0
                                        -ml-8 w-8 h-24 mt-14
                                        bg-red-500 hover:bg-red-400
                                        rounded-l-md cursor-pointer
                                        text-gray-300
                                        [writing-mode:vertical-lr] [text-orientation:upright]"
                                        @click="open = false"
                                    >
                                        RULE
                                    </div>
                                </TransitionChild> -->
                                <div class="flex h-full">
                                    <Card title="Rules" class="overflow-y-auto">
                                        <div class="flex flex-col space-y-1 w-full">
                                            <div
                                                v-for="transformation in transformations"
                                                :key="transformation"
                                                class="flex cursor-pointer rounded-lg px-4 py-2 shadow-md focus:outline-none select-none
                                                transition duration-75
                                                bg-white bg-opacity-10 hover:bg-opacity-20"
                                                @click="toggleTransformation(transformation)"
                                            >
                                                <div class="flex-1">
                                                    {{ transformation }}
                                                </div>
                                                <svg
                                                    class="h-6 w-6"
                                                    :class="{
                                                        'text-green-500 dark:text-green-600': enabledTransformations.includes(transformation),
                                                        'text-black dark:text-gray-900': !enabledTransformations.includes(transformation),
                                                    }"
                                                    viewBox="0 0 24 24"
                                                    fill="none"
                                                >
                                                    <circle cx="12" cy="12" r="12" fill="currentColor" fill-opacity="1" />
                                                    <path
                                                        v-if="enabledTransformations.includes(transformation)"
                                                        d="M7 13l3 3 7-7" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"
                                                    />
                                                </svg>
                                            </div>
                                        </div>
                                    </Card>
                                </div>
                            </DialogPanel>
                        </TransitionChild>
                    </div>
                </div>
            </div>
        </Dialog>
    </TransitionRoot>
</template>
