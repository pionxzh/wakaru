import { Suspense } from 'react'
import { FileTree } from './FileTree'

export function LeftExpander() {
    return (
        <div
            className="h-full"
            style={{ backgroundColor: '#252526' }}
        >
            <Suspense fallback={<div>Loading...</div>}>
                <FileTree />
            </Suspense>
        </div>
    )
}
