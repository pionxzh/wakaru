import path from 'node:path'
import { DockviewReact } from 'dockview'
import { Suspense } from 'react'
import { useEventSubscriber } from '../hooks/useEventEmitter'
import { Editor } from './Editor'
import type { DockviewReadyEvent, IDockviewPanelProps, IGridviewPanelProps, PanelCollection } from 'dockview'

const dockComponents: PanelCollection<IDockviewPanelProps> = {
    editor: (props: IDockviewPanelProps<{ path: string }>) => (
        <Suspense fallback={<div>Loading...</div>}>
            <Editor path={props.params.path} />
        </Suspense>
    ),
}

export function Dock(props: IGridviewPanelProps) {
    useEventSubscriber('file:open', ({ path: _path }) => {
        const event = props.params.api.current as DockviewReadyEvent | undefined
        if (!event) return

        const panel = event.api.getPanel(_path)
        if (panel) {
            panel.focus?.()
            return
        }

        const fileName = path.basename(_path)
        event.api.addPanel({
            id: _path,
            title: fileName,
            component: 'editor',
            params: { path: _path },
        })
    })

    const onReady = (event: DockviewReadyEvent) => {
        props.params.api.current = event
    }

    return (
        <DockviewReact
            components={dockComponents}
            onReady={onReady}
        />
    )
}
