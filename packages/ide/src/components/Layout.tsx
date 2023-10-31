import { GridviewReact, LayoutPriority, Orientation } from 'dockview'
import { useAtomValue } from 'jotai'
import { useCallback, useEffect } from 'react'
import { fsAtom, unpackedAtom } from '../atoms/fs'
import { useDarkMode } from '../hooks/useDarkMode'
import { Dock } from './Dock'
import { Footer } from './Footer'
import { Header } from './Header'
import { LeftExpander } from './LeftExpander'
import { SideBar } from './SideBar'
import type { GridviewApi, GridviewReadyEvent } from 'dockview'

const components = {
    'header': Header,
    'footer': Footer,
    'sidebar': SideBar,
    'left-expander': LeftExpander,
    'dock': Dock,
}

export function Layout() {
    const { isDarkMode } = useDarkMode()

    useEffect(() => {
        if (isDarkMode) {
            document.documentElement.classList.add('dark')
        }
        else {
            document.documentElement.classList.remove('dark')
        }
    }, [isDarkMode])

    const fs = useAtomValue(fsAtom)
    const { modules, moduleIdMapping } = useAtomValue(unpackedAtom)
    Promise.all(modules.map(async (module) => {
        const fileName = moduleIdMapping[module.id] ?? `module-${module.id}.js`
        await fs.promises.writeFile(`/${fileName}`, module.code, {
            encoding: 'utf8',
            flag: 'w',
        })
    }))

    const onReady = useCallback((event: GridviewReadyEvent) => {
        const api = event.api
        createGridApi(api)
    }, [])

    return (
        <GridviewReact
            components={components}
            onReady={onReady}
            className={isDarkMode ? 'dockview-theme-dark' : 'dockview-theme-light'}
        />
    )
}

function createGridApi(api: GridviewApi) {
    api.orientation = Orientation.VERTICAL

    api.addPanel({
        id: 'footer-id',
        component: 'footer',
        minimumHeight: 24,
        maximumHeight: 24,
    })

    api.addPanel({
        id: 'dock',
        component: 'dock',
        minimumWidth: 100,
        minimumHeight: 100,
        /**
         * it's important to give the main content a high layout priority as we
         * want the main layout to have priority when allocating new space
         */
        priority: LayoutPriority.High,
        params: {
            api,
        },
    })

    api.addPanel({
        id: 'explore',
        component: 'left-expander',
        minimumWidth: 200,
        size: 200,
        snap: true,
        position: { referencePanel: 'dock', direction: 'left' },
    })

    api.addPanel({
        id: 'sidebar-id',
        component: 'sidebar',
        minimumWidth: 30,
        maximumWidth: 30,
        size: 30,
        position: { referencePanel: 'explore', direction: 'left' },
    })

    api.addPanel({
        id: 'header-id',
        component: 'header',
        minimumHeight: 30,
        maximumHeight: 30,
    })
}
