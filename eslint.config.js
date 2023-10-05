import pionxzh from '@pionxzh/eslint-config'

export default pionxzh(
    {
        typescript: true,
        react: false,
        vue: true,
        yaml: false,
        ignores: [
            '**/node_modules/**',
            '**/dist/**',
        ],
    },
    {
        rules: {
            'no-console': 'warn',
            'ts/ban-ts-comment': 'off',
            'pionxzh/top-level-function': 'off',
            'pionxzh/consistent-list-newline': 'off',

            // the following rules are causing performance issues?
            'pionxzh/generic-spacing': 'off',
            'pionxzh/named-tuple-spacing': 'off',
            'pionxzh/no-cjs-exports': 'off',
        },
    },
)
