<script setup lang="ts">
import Header from './components/Header.vue'
import SideBar from './components/SideBar.vue'
import Main from './Main.vue'
</script>

<template>
    <div class="h-screen text-gray-800 dark:text-gray-100">
        <Header />

        <div>
            <SideBar />
            <Suspense>
                <Main />
            </Suspense>
        </div>
    </div>
</template>
