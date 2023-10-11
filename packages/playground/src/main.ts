import './style.css'
import { library } from '@fortawesome/fontawesome-svg-core'
import { faJs, faNpm } from '@fortawesome/free-brands-svg-icons'
import { faChevronDown, faChevronUp, faCode, faShareNodes } from '@fortawesome/free-solid-svg-icons'
import { FontAwesomeIcon } from '@fortawesome/vue-fontawesome'
import { createApp } from 'vue'
import App from './App.vue'
import { router } from './router'

library.add(faChevronDown, faChevronUp, faCode, faShareNodes)
library.add(faJs, faNpm)

const app = createApp(App)
app.use(router)
app.component('FontAwesomeIcon', FontAwesomeIcon)
app.mount('#app')
