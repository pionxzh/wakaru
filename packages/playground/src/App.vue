<script setup lang="ts">
import SideBar from './components/SideBar.vue'

// const isXL = useMediaQuery('(min-width: 1280px)')
</script>

<template>
    <div class="flex overflow-x-hidden h-screen">
        <SideBar />

        <div class="flex-1 text-gray-800 dark:text-gray-100" style="padding-left: 16rem;">
            <header
                class="flex items-center p-4 text-semibold
      bg-gray-50 dark:bg-gray-800"
            >
                <button class="p-1 mr-4" @click="sidebarOpen = !sidebarOpen">
                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor" class="h-6 w-6">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16" />
                    </svg>
                </button>
                Header
            </header>
            <main class="">
                <router-view :key="$route.path" />
            </main>
        </div>
    </div>
</template>
