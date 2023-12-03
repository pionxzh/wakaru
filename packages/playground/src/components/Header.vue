<script setup lang="ts">
import logo from '/icon.png?url'
import DarkModeBtn from './DarkModeBtn.vue'
</script>

<template>
    <header
        class="flex h-16 items-center py-4 pl-6 pr-8
        text-base text-semibold
        bg-[#e5e5e5] dark:bg-gray-800
        border-b border-gray-300 dark:border-gray-700"
    >
        <img :src="logo" alt="logo" class="w-6 h-6 mr-2">

        Wakaru
        <div class="flex-1" />
        <div class="flex items-center space-x-4">
            <DarkModeBtn />
            <a
                href="https://github.com/pionxzh/wakaru"
                target="_blank"
                class="hover:underline"
            >
                GitHub
            </a>
        </div>
    </header>
</template>
