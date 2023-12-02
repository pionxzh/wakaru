<script setup lang="ts">
import { useSortable } from '@vueuse/integrations/useSortable'
import { shallowRef } from 'vue'
import Card from '../components/Card.vue'
import { useTransformationRules } from '../composables/useTransformationRules'

const { allRules, enabledRules, toggleRule } = useTransformationRules()
const rulesList = shallowRef<HTMLElement | null>(null)
useSortable(rulesList, allRules)
</script>

<template>
    <Card
        title="Rules"
        description="Drag and drop rule name to tweak rules order."
        class="h-full overflow-y-auto"
    >
        <div ref="rulesList" class="flex flex-col space-y-1 w-full">
            <div
                v-for="rule in allRules"
                :key="rule.id"
                class="flex cursor-pointer rounded-lg px-4 py-2 shadow-md focus:outline-none select-none
                        transition duration-75
                        bg-white bg-opacity-10 hover:bg-opacity-20"
                @click="toggleRule(rule.id)"
            >
                <div class="flex-1">
                    {{ rule.name }}
                </div>
                <svg
                    class="h-6 w-6"
                    :class="{
                        'text-green-500 dark:text-green-600': enabledRules.includes(rule),
                        'text-black dark:text-gray-900': !enabledRules.includes(rule),
                    }"
                    viewBox="0 0 24 24"
                    fill="none"
                >
                    <circle cx="12" cy="12" r="12" fill="currentColor" fill-opacity="1" />
                    <path
                        v-if="enabledRules.includes(rule)"
                        d="M7 13l3 3 7-7" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"
                    />
                </svg>
            </div>
        </div>
    </Card>
</template>
