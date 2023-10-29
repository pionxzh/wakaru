import Monaco from '@monaco-editor/react'
import { useAtomValue } from 'jotai'
import { AutoTypings, LocalStorageCache } from 'monaco-editor-auto-typings/custom-editor'
import { fsAtom } from '../atoms/fs'
import { useDarkMode } from '../hooks/useDarkMode'
import type { OnMount } from '@monaco-editor/react'

export interface EditorProps {
    path: string
}

const sourceCache = new LocalStorageCache()

export function Editor(props: EditorProps) {
    const { isDarkMode } = useDarkMode()
    const fs = useAtomValue(fsAtom)

    const handleMount: OnMount = async (editor, monaco) => {
        // enable auto typings
        AutoTypings.create(editor, { monaco, sourceCache, fileRootPath: './' })

        // read the file
        let content = ''
        try {
            content = await fs.promises.readFile(props.path, 'utf-8')
        }
        catch (e) { }
        editor.setValue(content)

        // unlock the editor
        editor.updateOptions({ readOnly: false })
    }

    return (
        <Monaco
            path={props.path}
            theme={isDarkMode ? 'vs-dark' : 'vs-light'}
            options={{
                readOnly: true,
                padding: { top: 10 },
            }}
            onMount={handleMount}
            // onChange={value => fs.writeFile(props.path, value || '', 'utf-8')}
        />
    )
}
