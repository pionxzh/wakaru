<script setup lang="ts">
import { TransitionRoot } from '@headlessui/vue'
import { useSortable } from '@vueuse/integrations/useSortable'
import { computed, shallowRef, watch } from 'vue'
import { useRoute } from 'vue-router'
import Card from '../components/Card.vue'
import CodemirrorEditor from '../components/CodemirrorEditor.vue'
import ShareBtn from '../components/ShareBtn.vue'
import useState from '../composables/shared/useState'
import { encodeOption } from '../composables/url'
import { useCodemod } from '../composables/useCodemod'
import { useModule } from '../composables/useModule'
import { useModuleMapping } from '../composables/useModuleMapping'
import { useModuleMeta } from '../composables/useModuleMeta'
import { useTransformationRules } from '../composables/useTransformationRules'

const { params: { id } } = useRoute()
const { module, setModule } = useModule(id as string)
const { moduleMeta } = useModuleMeta()
const { moduleMapping } = useModuleMapping()

const [openSideBar, setOpenSideBar] = useState(false)

const { disabledRules, setDisabledRules, allRules } = useTransformationRules()
const enabledRules = computed(() => allRules.value.filter(t => !disabledRules.value.includes(t)))
function toggleRules(transformation: string) {
    if (disabledRules.value.includes(transformation)) {
        setDisabledRules(disabledRules.value.filter(t => t !== transformation))
    }
    else {
        setDisabledRules([...disabledRules.value, transformation])
    }
}

const rulesList = shallowRef<HTMLElement | null>(null)
useSortable(rulesList, allRules)

const { transform } = useCodemod()
const moduleName = computed(() => moduleMapping.value[module.value.id])

watch([enabledRules, () => module.value.code], async () => {
    const result = await transform(moduleName.value, module.value, enabledRules.value, moduleMeta.value, moduleMapping.value)
    setModule({ ...module.value, transformed: result.transformed })
}, { immediate: true })

const onClick = () => {
    const hash = encodeOption({
        code: module.value.code,
        rules: disabledRules.value.length ? disabledRules.value : undefined,
        mapping: moduleMapping.value,
        meta: moduleMeta.value,
    })
    const shareUrl = `${location.origin}/#${hash}`
    navigator.clipboard.writeText(shareUrl)
    // eslint-disable-next-line no-alert
    alert('Copied to clipboard!')
}
</script>

<template>
    <div class="flex flex-row h-full">
        <Card class="border-x border-gray-700">
            <template #title>
                Source
                <ShareBtn @click="onClick" />
            </template>
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
            <Card
                title="Rules"
                description="Drag and drop rule name to tweak rules order."
                class="h-full overflow-y-auto"
            >
                <div ref="rulesList" class="flex flex-col space-y-1 w-full">
                    <div
                        v-for="rule in allRules"
                        :key="rule"
                        class="flex cursor-pointer rounded-lg px-4 py-2 shadow-md focus:outline-none select-none
                            transition duration-75
                            bg-white bg-opacity-10 hover:bg-opacity-20"
                        @click="toggleRules(rule)"
                    >
                        <div class="flex-1">
                            {{ rule }}
                        </div>
                        <svg
                            class="h-6 w-6"
                            :class="{
                                'text-green-500 dark:text-green-600': enabledRules.includes(rule),
                                'text-black dark:text-gray-900': !enabledRules.includes(rule),
                            }"
                            viewBox="0 0 24 24"
                            fill="none"
                        >
                            <circle cx="12" cy="12" r="12" fill="currentColor" fill-opacity="1" />
                            <path
                                v-if="enabledRules.includes(rule)"
                                d="M7 13l3 3 7-7" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"
                            />
                        </svg>
                    </div>
                </div>
            </Card>
        </div>
    </div>
</template>
