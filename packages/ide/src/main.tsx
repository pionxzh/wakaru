import React from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App.tsx'

import './index.css'
import 'react-complex-tree/lib/style-modern.css'
import 'dockview/dist/styles/dockview.css'

const root = createRoot(document.getElementById('root')!)
root.render(
    <React.StrictMode>
        <App />
    </React.StrictMode>,
)
