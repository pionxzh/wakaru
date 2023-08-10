<script setup lang="ts">
import useState from '../composables/shared/useState'

defineProps<{
    accept: string
}>()

const emit = defineEmits<{
    (e: 'upload', file: File): void
}>()

const [dragging, setDragging] = useState(false)

function onInput(event: Event) {
    const file = (event.target as HTMLInputElement).files?.[0]
    if (file) emit('upload', file)
}

function onDrop(event: DragEvent) {
    setDragging(false)
    const file = event.dataTransfer?.files?.[0]
    if (file) emit('upload', file)
}
</script>

<template>
    <div
        class="flex justify-center items-center w-full"
        @dragenter.prevent="setDragging(true)"
        @dragover.prevent="setDragging(true)"
        @dragleave.prevent="setDragging(false)"
        @drop.prevent="onDrop"
    >
        <label
            for="dropzone-zone"
            class="flex flex-col justify-center items-center w-full h-28 rounded-lg border-2 cursor-pointer transition
            bg-gray-50 dark:bg-gray-700
            hover:bg-gray-100 dark:hover:bg-gray-600
            border-dashed border-gray-300 dark:border-gray-600 dark:hover:border-gray-500"
            :class="{
                'border-blue-500 dark:border-blue-500': dragging,
            }"
        >
            <div class="flex flex-col justify-center items-center pt-5 pb-6">
                <svg aria-hidden="true" class="mb-1 w-10 h-10 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" /></svg>
                <p class="text-sm text-gray-500 dark:text-gray-400"><span class="font-semibold">Click to upload</span> or drag and drop</p>
            </div>
            <input id="dropzone-zone" class="hidden" type="file" :accept="accept" @input="onInput">
        </label>
    </div>
</template>
