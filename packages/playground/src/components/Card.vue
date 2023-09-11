<script setup lang="ts">
defineProps<{
    title?: string
    description?: string
}>()
</script>

<template>
    <div
        class="relative px-6 py-4 w-full border shadow-md dark:border-gray-700
        text-gray-900 dark:text-white
        bg-light dark:bg-gray-800"
    >
        <slot name="prepend" />
        <h5 v-if="title" class="mb-2 text-2xl text-center font-bold text-gray-900 dark:text-white">
            {{ title }}
        </h5>
        <p v-if="description" class="mb-5 text-base text-center text-gray-500 sm:text-lg dark:text-gray-400">
            {{ description }}
        </p>
        <div class="justify-center items-center space-y-4 sm:flex sm:space-y-0 sm:space-x-4">
            <slot />
        </div>
    </div>
</template>
