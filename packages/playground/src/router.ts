import { createRouter, createWebHistory } from 'vue-router'

import CodeEditor from './pages/CodeEditor.vue'
import ModuleMapping from './pages/ModuleMapping.vue'
import Uploader from './pages/Uploader.vue'

export const router = createRouter({
    history: createWebHistory(),
    routes: [
        { name: 'source', path: '/', component: Uploader },
        { name: 'module-mapping', path: '/mapping/', component: ModuleMapping },
        { name: 'file', path: '/file/:id', component: CodeEditor },
    ],
})
