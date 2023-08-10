<script setup lang="ts">
import { ref, watch } from 'vue'
import Card from '../components/Card.vue'
import CodemirrorEditor from '../components/CodemirrorEditor.vue'
import { useModuleMapping } from '../composables/useModuleMapping'

const { moduleMapping, setModuleMapping } = useModuleMapping()
const code = ref(JSON.stringify(moduleMapping.value, null, 2))

watch(code, (value) => {
    try {
        setModuleMapping(JSON.parse(value))
    }
    catch (e) {
        console.error(e)
    }
})
</script>

<template>
    <Card
        title="Module Mapping"
        description="👀 Take a look at each module and give it a good name."
    >
        <div class="flex flex-col w-full">
            <div class="flex-1">
                <CodemirrorEditor
                    v-model="code"
                    :style="{
                        height: '400px;',
                    }"
                />
            </div>
        </div>
    </Card>
</template>
