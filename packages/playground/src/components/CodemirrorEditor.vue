<script setup lang="ts">
import { javascript } from '@codemirror/lang-javascript'
import { oneDark } from '@codemirror/theme-one-dark'
import { EditorView } from '@codemirror/view'
import { useDark } from '@vueuse/core'
import { computed } from 'vue'
import { Codemirror } from 'vue-codemirror'
import type { CSSProperties, PropType } from 'vue'

defineProps({
    modelValue: {
        type: String,
        required: true,
    },
    placeholder: {
        type: String,
        default: 'Code goes here...',
    },
    disabled: {
        type: Boolean,
        default: false,
    },
    autofocus: {
        type: Boolean,
        default: false,
    },
    style: {
        type: Object as PropType<CSSProperties>,
        default: () => ({}),
    },
})

const emit = defineEmits(['update:modelValue'])

const darkMode = useDark({ storageKey: 'color-scheme' })

const extensions = computed(() => [
    javascript(),
    ...(darkMode.value ? [oneDark] : []),
    EditorView.lineWrapping,
])
</script>

<template>
    <Codemirror
        :model-value="modelValue"
        :placeholder="placeholder"
        :style="style"
        :autofocus="autofocus"
        :disabled="disabled"
        indent-with-tab
        :tab-size="2"
        :extensions="extensions"
        spellcheck="false"
        autocorrect="off"
        autocapitalize="off"
        translate="no"
        data-gramm="false"
        data-gramm_editor="false"
        data-enable-grammarly="false"
        @update:model-value="emit('update:modelValue', $event)"
    />
</template>
