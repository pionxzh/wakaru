/// <reference types="node" />
/// <reference lib="dom" />
import { AsyncKeyValueROTransaction, AsyncKeyValueRWTransaction, AsyncKeyValueStore, AsyncKeyValueFileSystem } from '../generic/key_value_filesystem';
import { type BackendOptions } from './backend';
/**
 * @hidden
 */
export declare class IndexedDBROTransaction implements AsyncKeyValueROTransaction {
    tx: IDBTransaction;
    store: IDBObjectStore;
    constructor(tx: IDBTransaction, store: IDBObjectStore);
    get(key: string): Promise<Buffer>;
}
/**
 * @hidden
 */
export declare class IndexedDBRWTransaction extends IndexedDBROTransaction implements AsyncKeyValueRWTransaction, AsyncKeyValueROTransaction {
    constructor(tx: IDBTransaction, store: IDBObjectStore);
    /**
     * @todo return false when add has a key conflict (no error)
     */
    put(key: string, data: Buffer, overwrite: boolean): Promise<boolean>;
    del(key: string): Promise<void>;
    commit(): Promise<void>;
    abort(): Promise<void>;
}
export declare class IndexedDBStore implements AsyncKeyValueStore {
    private db;
    private storeName;
    static Create(storeName: string, indexedDB: IDBFactory): Promise<IndexedDBStore>;
    constructor(db: IDBDatabase, storeName: string);
    name(): string;
    clear(): Promise<void>;
    beginTransaction(type: 'readonly'): AsyncKeyValueROTransaction;
    beginTransaction(type: 'readwrite'): AsyncKeyValueRWTransaction;
}
export declare namespace IndexedDBFileSystem {
    /**
     * Configuration options for the IndexedDB file system.
     */
    interface Options {
        /**
         * The name of this file system. You can have multiple IndexedDB file systems operating at once, but each must have a different name.
         */
        storeName?: string;
        /**
         * The size of the inode cache. Defaults to 100. A size of 0 or below disables caching.
         */
        cacheSize?: number;
        /**
         * The IDBFactory to use. Defaults to `globalThis.indexedDB`.
         */
        idbFactory?: IDBFactory;
    }
}
/**
 * A file system that uses the IndexedDB key value file system.
 */
export declare class IndexedDBFileSystem extends AsyncKeyValueFileSystem {
    static readonly Name = "IndexedDB";
    static Create: any;
    static readonly Options: BackendOptions;
    static isAvailable(idbFactory?: IDBFactory): boolean;
    constructor({ cacheSize, storeName, idbFactory }: IndexedDBFileSystem.Options);
}
