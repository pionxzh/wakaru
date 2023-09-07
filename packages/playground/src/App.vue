<script setup lang="ts">
import DarkModeBtn from './components/DarkModeBtn.vue'
import SideBar from './components/SideBar.vue'

// const isXL = useMediaQuery('(min-width: 1280px)')
</script>

<template>
    <div class="flex overflow-x-hidden h-screen">
        <div class="flex-1 text-gray-800 dark:text-gray-100">
            <header class="flex items-center py-4 pl-6 pr-8 text-base text-semibold bg-gray-50 dark:bg-gray-800">
                {{ "<LOGO>" }}
                Unminify Kit
                <div class="flex-1" />
                <div class="flex items-center space-x-4">
                    <DarkModeBtn />
                    <a href="https://github.com/" class="hover:underline">GitHub</a>
                </div>
            </header>
            <SideBar />
            <main style="padding-left: 16rem;">
                <router-view :key="$route.path" />
            </main>
        </div>
    </div>
</template>
