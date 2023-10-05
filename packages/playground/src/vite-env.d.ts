/* eslint-disable ts/ban-types */
/* eslint-disable no-restricted-syntax */
/// <reference types="vite/client" />

declare module '*.vue' {
    import type { DefineComponent } from 'vue'

    const component: DefineComponent<{}, {}, any>
    export default component
}
