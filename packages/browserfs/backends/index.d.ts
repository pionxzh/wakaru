import { FileSystemAccessFileSystem as FileSystemAccess } from './FileSystemAccess';
import { FolderAdapter } from './FolderAdapter';
import { InMemoryFileSystem as InMemory } from './InMemory';
import { IndexedDBFileSystem as IndexedDB } from './IndexedDB';
import { BackendConstructor } from './backend';
export declare const backends: {
    [backend: string]: BackendConstructor;
};
export { FileSystemAccess, FolderAdapter, InMemory, IndexedDB, };
