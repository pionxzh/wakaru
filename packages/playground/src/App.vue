<script setup lang="ts">
import Header from './components/Header.vue'
import SideBar from './components/SideBar.vue'
import { KEY_TRANSFORMATIONS } from './const'

// clear deprecated localStorage items
window.localStorage.removeItem(KEY_TRANSFORMATIONS)
</script>

<template>
    <div class="h-screen text-gray-800 dark:text-gray-100">
        <Header />

        <div>
            <SideBar />
            <main style="height: calc(100vh - 4rem); padding-left: 16rem;">
                <router-view :key="$route.path" />
            </main>
        </div>
    </div>
</template>
