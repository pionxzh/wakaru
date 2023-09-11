<script setup lang="ts">
import { TransitionRoot } from '@headlessui/vue'
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
        <Card title="Source" class="border-x border-gray-700">
            <div class="w-full">
                <CodemirrorEditor
                    :model-value="module.code"
                    :style="{
                        height: 'calc(100vh - 9rem);',
                    }"
                    @update:model-value="setModule({ ...module, code: $event })"
                />
            </div>
        </Card>
        <Card title="Transformed" class="border-x border-gray-700">
            <div class="w-full">
                <CodemirrorEditor
                    :model-value="module?.transformed"
                    :style="{
                        height: 'calc(100vh - 9rem);',
                    }"
                />
            </div>
        </Card>
    </div>

    <TransitionRoot
        :show="openSideBar"
        as="template"
        enter="ease-in-out duration-500"
        enter-from="opacity-0" enter-to="opacity-100"
        leave="ease-in-out duration-500"
        leave-from="opacity-100"
        leave-to="opacity-0"
    >
        <div
            class="fixed inset-0 bg-gray-500 dark:bg-black bg-opacity-75 dark:bg-opacity-50 transition-opacity"
            aria-hidden="true"
            @click="setOpenSideBar(false)"
        />
    </TransitionRoot>

    <div
        class="fixed inset-0 overflow-hidden w-screen h-screen pointer-events-none"
    >
        <div
            class="absolute top-0 right-0 w-md h-full pointer-events-auto
            transform transition ease-in-out duration-500 translate-x-0"
            :class="{
                'translate-x-full': !openSideBar,
            }"
        >
            <div
                class="absolute flex justify-center items-center top-0 left-0
            w-8 h-28 -translate-x-full mt-16 rounded-l-xl
            text-gray-300 bg-red-500 hover:bg-red-400 tracking-[-0.4em]
            shadow-md
            pointer-events-auto cursor-pointer transition-colors
            [writing-mode:vertical-lr] [text-orientation:upright]"
                @click="setOpenSideBar((v) => !v)"
            >
                <FontAwesomeIcon
                    icon="fa-solid fa-chevron-down"
                    class="flex-shrink-0 w-4 h-4 rotate-90 text-orange-400"
                    :class="{
                        '-rotate-90': openSideBar,
                    }"
                />
                RULE
            </div>
            <Card title="Rules" class="h-full overflow-y-auto">
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
    </div>
</template>
