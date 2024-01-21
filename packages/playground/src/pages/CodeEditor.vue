<script setup lang="ts">
import { TransitionRoot } from '@headlessui/vue'
import { watchDebounced } from '@vueuse/core'
import { useAtom, useAtomValue, useSetAtom } from 'jotai-vue'
import { useRoute } from 'vue-router'
import { getModuleAtom, moduleMappingAtom, moduleMetaAtom, unminifyModuleAtom } from '../atoms/module'
import { disabledRuleIdsAtom, enabledRuleIdsAtom } from '../atoms/rule'
import Card from '../components/Card.vue'
import CodemirrorEditor from '../components/CodemirrorEditor.vue'
import RuleList from '../components/RuleList.vue'
import ShareBtn from '../components/ShareBtn.vue'
import useState from '../composables/shared/useState'
import { encodeOption } from '../composables/url'
import { SharedDataVersion } from '../const'

const [openSideBar, setOpenSideBar] = useState(false)

const { params: { id } } = useRoute()
const moduleAtom = getModuleAtom(id as string)
const [module, setModule] = useAtom(moduleAtom)
const moduleMeta = useAtomValue(moduleMetaAtom)
const moduleMapping = useAtomValue(moduleMappingAtom)

const enabledRuleIds = useAtomValue(enabledRuleIdsAtom)
const disabledRuleIds = useAtomValue(disabledRuleIdsAtom)

const unminifyModule = useSetAtom(unminifyModuleAtom)

watchDebounced(
    [enabledRuleIds, () => module.value?.code],
    () => unminifyModule(moduleAtom),
    { immediate: true, debounce: 500 },
)

const copySharableUrl = () => {
    const hash = encodeOption({
        version: SharedDataVersion,
        code: module.value.code,
        rules: disabledRuleIds.value.length ? disabledRuleIds.value : undefined,
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
                <ShareBtn @click="copySharableUrl" />
            </template>
            <div class="w-full">
                <CodemirrorEditor
                    :model-value="module.code"
                    :style="{
                        height: 'calc(100vh - 9rem);',
                    }"
                    @update:model-value="setModule({ code: $event })"
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
                    class="flex-shrink-0 w-4 h-4 text-orange-400"
                    :class="{
                        'rotate-90': !openSideBar,
                        '-rotate-90': openSideBar,
                    }"
                />
                RULE
            </div>
            <RuleList />
        </div>
    </div>
</template>
