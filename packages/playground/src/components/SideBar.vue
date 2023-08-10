<script setup lang="ts">
import { computed } from 'vue'
import useState from '../composables/shared/useState'
import { useFileIds } from '../composables/useFileIds'
import { useModuleMapping } from '../composables/useModuleMapping'
import DarkModeBtn from './DarkModeBtn.vue'
import InputBox from './InputBox.vue'
import Separator from './Separator.vue'

type FileId = number | string

const [editingFileId, useEditingFileId] = useState<FileId>(-1)
const { fileIds } = useFileIds()
const { moduleMapping } = useModuleMapping()

const files = computed(() => {
    return fileIds.value.map((id) => {
        return {
            id,
            name: moduleMapping.value[id] ?? `modules-${id}.js`,
        }
    })
})

function cancelRename() {
    useEditingFileId(-1)
}

function rename(fileId: FileId, e: Event) {
    useEditingFileId(-1)
    const newName = (e.target as HTMLInputElement).value
    if (newName && newName !== moduleMapping.value[fileId]) {
        moduleMapping.value = {
            ...moduleMapping.value,
            [fileId]: newName,
        }
    }
}
</script>

<template>
    <aside
        class="fixed h-screen w-64 flex flex-shrink-0 flex-col border-r overflow-y-auto select-none
        transition-all duration-300
      bg-gray-50 dark:bg-gray-800"
    >
        <DarkModeBtn class="my-4 mx-auto" />

        <ul class="relative pb-6">
            <li class="cursor-pointer">
                <router-link
                    :to="{ name: 'source' }"
                    class="flex items-center pl-6 pr-2 py-2 w-full text-base font-normal rounded-md transition duration-75
                    text-gray-900 dark:text-white
                    hover:bg-gray-200 dark:hover:bg-gray-700"
                    exact-active-class="active bg-gray-200 dark:bg-gray-700"
                >
                    <FontAwesomeIcon
                        icon="fa-brands fa-js"
                        class="flex-shrink-0 w-5 h-5 text-gray-500 dark:text-gray-400"
                    />
                    <span class="flex-1 ml-2 text-left whitespace-nowrap">
                        Source
                    </span>
                </router-link>
            </li>
            <li class="cursor-pointer">
                <router-link
                    :to="{ name: 'module-mapping' }"
                    class="flex items-center pl-6 pr-2 py-2 w-full text-base font-normal rounded-md transition duration-75
                    text-gray-900 dark:text-white
                    hover:bg-gray-200 dark:hover:bg-gray-700"
                    exact-active-class="active bg-gray-200 dark:bg-gray-700"
                >
                    <FontAwesomeIcon
                        icon="fa-solid fa-code"
                        class="flex-shrink-0 w-5 h-5 text-gray-500 dark:text-gray-400"
                    />
                    <span class="flex-1 ml-2 text-left whitespace-nowrap">
                        Module Mapping
                    </span>
                </router-link>
            </li>

            <Separator class="px-3 py-1.5" />

            <li
                v-for="file in files"
                :key="file.id"
                :title="file.name"
                class="cursor-pointer"
                @dblclick="editingFileId = file.id"
            >
                <router-link
                    :to="{ name: 'file', params: { id: file.id } }"
                    class="flex items-center pl-6 pr-2 py-1 w-full text-base font-normal transition duration-75
                    text-gray-900 dark:text-white
                    hover:bg-gray-200 dark:hover:bg-gray-700"
                    :class="{ 'bg-gray-100 dark:bg-gray-700': editingFileId === file.id }"
                    exact-active-class="active bg-gray-100 dark:bg-gray-700"
                >
                    <FontAwesomeIcon
                        v-if="file.name === 'package.json'"
                        icon="fa-brands fa-npm"
                        class="flex-shrink-0 w-5 h-5 text-gray-500 dark:text-gray-400"
                    />
                    <FontAwesomeIcon
                        v-if="file.name.endsWith('.json')"
                        icon="fa-solid fa-file-code"
                        class="flex-shrink-0 w-5 h-5 text-gray-500 dark:text-gray-400"
                    />
                    <FontAwesomeIcon
                        v-if="file.name.endsWith('.js')"
                        icon="fa-brands fa-js"
                        class="flex-shrink-0 w-5 h-5 text-gray-500 dark:text-gray-400"
                    />
                    <FontAwesomeIcon
                        v-else
                        icon="fa-solid fa-code"
                        class="flex-shrink-0 w-5 h-5 text-gray-500 dark:text-gray-400"
                    />
                    <template v-if="editingFileId === file.id">
                        <div
                            class="absolute w-full h-full left-0 right-0 top-0 bottom-0 z-0 cursor-default
                            bg-gray-800 dark:bg-black
                            bg-opacity-20 dark:opacity-50"
                        />
                        <InputBox
                            :model-value="file.name"
                            auto-select
                            class="flex-1 ml-2 text-left whitespace-nowrap z-10
                            bg-white dark:bg-gray-900
                            border border-gray-300 dark:border-gray-700"
                            @keyup.enter="rename(file.id, $event)"
                            @keyup.esc="cancelRename"
                            @blur="rename(file.id, $event)"
                        />
                    </template>
                    <span v-else class="flex-1 ml-2 text-left whitespace-nowrap">
                        {{ file.name }}
                    </span>
                </router-link>
            </li>
        </ul>
    </aside>
</template>
