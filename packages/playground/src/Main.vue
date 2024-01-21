<script setup lang="ts">
import { prepare } from './atoms/module'

await prepare()
</script>

<template>
    <main style="height: calc(100vh - 4rem); padding-left: 16rem;">
        <router-view :key="$route.path" />
    </main>
</template>
