import { useAtom, useAtomValue } from 'jotai'
import { Tree, type TreeDataProvider, type TreeItem, UncontrolledTreeEnvironment } from 'react-complex-tree'
import { fileListAtom, fileListUpdateEffect } from '../atoms/files'
import { useDarkMode } from '../hooks/useDarkMode'
import { useEventPublisher } from '../hooks/useEventEmitter'
import { cn } from '../utils/utils'
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
        children: Object.keys(items),
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
            console.log('onRenameItem', item, name)
        },
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
