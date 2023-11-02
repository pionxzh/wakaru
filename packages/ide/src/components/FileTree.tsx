import { useAtom, useAtomValue } from 'jotai'
import { Tree, type TreeDataProvider, type TreeItem, UncontrolledTreeEnvironment } from 'react-complex-tree'
import { fileListAtom, fileListUpdateEffect } from '../atoms/files'
import { useDarkMode } from '../hooks/useDarkMode'
import { useEventPublisher } from '../hooks/useEventEmitter'
import { cn } from '../utils/utils'
import { CssIcon, IconFile, IconFolder, IconHtml, IconJavascript, IconNpm, IconReact, IconTypescript } from './icons'
import type { IDirent } from '../atoms/files'

export function FileTree() {
    const { isDarkMode } = useDarkMode()

    const fileList = useAtomValue(fileListAtom)
    useAtom(fileListUpdateEffect)
    const flattenedFileList = flattenFileList(fileList)

    const items = Object.fromEntries(flattenedFileList.map(f => [f.index, f]))

    items.root = {
        index: 'root',
        canMove: true,
        isFolder: true,
        children: fileList.map(f => f.name),
        data: { type: 'dir', name: 'root', path: '/', children: [] },
        canRename: false,
    }

    const dataProvider: TreeDataProvider<IDirent> = {
        async getTreeItem(id) {
            return items[id]
        },
        async getTreeItems(ids) {
            return ids.map(id => items[id])
        },
        async onChangeItemChildren(id, children) {
            items[id].children = children
        },
        // async onDidChangeTreeData(changedItemIds) {
        //     console.log(changedItemIds)
        //     return () => {}
        // },
        async onRenameItem(item, name) {
            // eslint-disable-next-line no-console
            console.log('onRenameItem', item, name)
        },
    }

    const renderItemTitle = ({ item }: { item: TreeItem<IDirent> }) => {
        const { isFolder = false } = item
        const ext = item.data.type === 'file' ? item.data.ext : ''
        return (
            <>
                <FileTreeItemIcon ext={ext} isFolder={isFolder} />
                {item.data.name}
            </>
        )
    }

    const openFile = useEventPublisher('file:open')
    const handlePrimaryAction = (item: TreeItem<IDirent>) => {
        openFile({ path: item.data.path })
    }

    const renameFile = useEventPublisher('file:rename')
    const handleRenameItem = (item: TreeItem, newName: string) => {
        renameFile({ path: item.data, newName })
    }

    return (
        <UncontrolledTreeEnvironment
            dataProvider={dataProvider}
            getItemTitle={item => item.data.name}
            viewState={{}}
            canSearch={false}
            renderItemTitle={renderItemTitle}
            onPrimaryAction={handlePrimaryAction}
            onRenameItem={handleRenameItem}
        >
            <div
                className={cn(
                    'h-full overflow-y-scroll text-vscode-sideBar-foreground bg-vscode-sideBar-background',
                    isDarkMode ? 'rct-dark' : 'rct-light',
                )}
            >
                <Tree
                    treeId="explore"
                    rootItem="root"
                />
            </div>
        </UncontrolledTreeEnvironment>
    )
}

function flattenFileList(fileList: IDirent[]): TreeItem<IDirent>[] {
    const files: TreeItem<IDirent>[] = []

    for (const file of fileList) {
        if (file.type === 'file') {
            files.push({
                index: file.name,
                canMove: true,
                isFolder: false,
                children: [],
                data: file,
                canRename: true,
            })
        }
        if (file.type === 'dir') {
            files.push({
                index: file.name,
                canMove: true,
                isFolder: true,
                children: file.children.map(c => c.name),
                data: file,
                canRename: true,
            })
            files.push(...flattenFileList(file.children))
        }
    }

    return files
}

function FileTreeItemIcon({
    ext,
    isFolder,
}: {
    ext: string
    isFolder: boolean
}) {
    const Icon = getIconComp(ext, isFolder)
    return <Icon />
}

function getIconComp(ext: string, isFolder: boolean) {
    if (isFolder) return IconFolder

    switch (ext) {
        case '.js':
            return IconJavascript
        case '.ts':
            return IconTypescript
        case '.jsx':
        case '.tsx':
            return IconReact
        case '.html':
            return IconHtml
        case '.css':
            return CssIcon
        case '.json':
            return IconNpm
        default:
            return IconFile
    }
}
