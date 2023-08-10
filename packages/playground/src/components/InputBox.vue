<script setup lang="ts">
import { onMounted, ref } from 'vue'

const props = defineProps({
    modelValue: {
        type: String,
        required: true,
    },
    type: {
        type: String,
        default: 'text',
    },
    autoFocus: {
        type: Boolean,
        default: false,
    },
    autoSelect: {
        type: Boolean,
        default: false,
    },
})

const emit = defineEmits(['update:modelValue'])

const inputRef = ref<HTMLInputElement>()

const onInput = (e: Event) => {
    const target = e.target as HTMLInputElement
    emit('update:modelValue', target.value)
}

onMounted(() => {
    if (props.autoFocus || props.autoSelect) {
        inputRef.value?.focus()
    }
    if (props.autoSelect) {
        inputRef.value?.setSelectionRange(0, inputRef.value.value.lastIndexOf('.'))
    }
})
</script>

<template>
    <input
        ref="inputRef"
        class="w-full"
        :type="type"
        :value="modelValue"
        @input="onInput"
    >
</template>
