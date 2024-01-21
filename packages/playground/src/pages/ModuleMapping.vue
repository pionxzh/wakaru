<script setup lang="ts">
import { useAtom } from 'jotai-vue'
import { ref, watch } from 'vue'
import { moduleMappingAtom } from '../atoms/module'
import Card from '../components/Card.vue'
import CodemirrorEditor from '../components/CodemirrorEditor.vue'

const [moduleMapping, setModuleMapping] = useAtom(moduleMappingAtom)
const code = ref(JSON.stringify(moduleMapping.value, null, 2))

watch(code, (value) => {
    try {
        setModuleMapping(JSON.parse(value))
    }
    catch (e) {
        console.error(e)
        // TODO: hint user that the JSON is invalid
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
                        height: 'calc(100vh - 12rem)',
                    }"
                />
            </div>
        </div>
    </Card>
</template>
