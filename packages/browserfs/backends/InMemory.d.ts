/// <reference types="node" />
import { SyncKeyValueStore, SimpleSyncStore, SyncKeyValueRWTransaction, SyncKeyValueFileSystem } from '../generic/key_value_filesystem';
import { type BackendOptions } from './backend';
/**
 * A simple in-memory key-value store backed by a JavaScript object.
 */
export declare class InMemoryStore implements SyncKeyValueStore, SimpleSyncStore {
    private store;
    name(): string;
    clear(): void;
    beginTransaction(type: string): SyncKeyValueRWTransaction;
    get(key: string): Buffer;
    put(key: string, data: Buffer, overwrite: boolean): boolean;
    del(key: string): void;
}
/**
 * A simple in-memory file system backed by an InMemoryStore.
 * Files are not persisted across page loads.
 */
export declare class InMemoryFileSystem extends SyncKeyValueFileSystem {
    static readonly Name = "InMemory";
    static Create: any;
    static readonly Options: BackendOptions;
    constructor();
}
