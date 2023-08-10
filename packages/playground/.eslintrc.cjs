module.exports = {
    root: true,
    globals: {
        defineProps: 'readonly',
        defineEmits: 'readonly',
        defineExpose: 'readonly',
        withDefaults: 'readonly',
    },
    extends: [
        '@pionxzh/eslint-config-vue',
    ],
    rules: {
        'no-console': 'warn',
    },
    overrides: [
        {
            files: ['*.vue'],
            rules: {
                'vue/html-indent': ['error', 4],
            },
        },
    ],
}
