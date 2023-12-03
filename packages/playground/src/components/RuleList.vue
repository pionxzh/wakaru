<script setup lang="ts">
import { moveArrayElement, useSortable } from '@vueuse/integrations/useSortable'
import { useAtom, useAtomValue, useSetAtom } from 'jotai-vue'
import { nextTick, ref, shallowRef } from 'vue'
import { allRulesAtom, enabledRulesAtom, resetRulesAtom, ruleOrderAtom, toggleRuleAtom } from '../atoms/rule'
import Card from '../components/Card.vue'

const allRules = useAtomValue(allRulesAtom)
const enabledRules = useAtomValue(enabledRulesAtom)
const toggleRule = useSetAtom(toggleRuleAtom)
const resetRules = useSetAtom(resetRulesAtom)

const renderKey = ref(0)
const reset = () => {
    resetRules()
    renderKey.value += 1
}

const rulesList = shallowRef<HTMLElement | null>(null)
const [ruleOrder, setRuleOrder] = useAtom(ruleOrderAtom)
useSortable(rulesList, ruleOrder, {
    animation: 150,
    direction: 'vertical',
    onUpdate({ oldIndex, newIndex }: { oldIndex: number; newIndex: number }) {
        const newRuleOrder = [...ruleOrder.value]
        moveArrayElement(newRuleOrder, oldIndex, newIndex)

        nextTick(() => {
            setRuleOrder(newRuleOrder)
        })
    },
})
</script>

<template>
    <Card
        description="Drag and drop rule name to tweak rules order."
        class="h-full overflow-y-auto"
    >
        <template #title>
            Rules
            <div class="absolute top-0 right-0">
                <button
                    class="flex w-fit text-white font-bold text-sm"
                    @click="reset"
                >
                    Reset
                </button>
            </div>
        </template>
        <div ref="rulesList" :key="renderKey" class="flex flex-col space-y-1 w-full">
            <div v-for="rule in allRules" :key="rule.id">
                <div
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
        </div>
    </Card>
</template>
