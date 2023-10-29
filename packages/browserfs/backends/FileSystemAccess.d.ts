/// <reference types="node" />
/// <reference lib="dom" />
import { Cred } from '../cred';
import { File, FileFlag } from '../file';
import { BaseFileSystem, FileSystemMetadata } from '../filesystem';
import { Stats } from '../stats';
import PreloadFile from '../generic/preload_file';
import { type BackendOptions } from './backend';
interface FileSystemAccessFileSystemOptions {
    handle: FileSystemDirectoryHandle;
}
export declare class FileSystemAccessFile extends PreloadFile<FileSystemAccessFileSystem> implements File {
    constructor(_fs: FileSystemAccessFileSystem, _path: string, _flag: FileFlag, _stat: Stats, contents?: Buffer);
    sync(): Promise<void>;
    close(): Promise<void>;
}
export declare class FileSystemAccessFileSystem extends BaseFileSystem {
    static readonly Name = "FileSystemAccess";
    static Create: any;
    static readonly Options: BackendOptions;
    static isAvailable(): boolean;
    private _handles;
    constructor({ handle }: FileSystemAccessFileSystemOptions);
    get metadata(): FileSystemMetadata;
    _sync(p: string, data: Buffer, stats: Stats, cred: Cred): Promise<void>;
    rename(oldPath: string, newPath: string, cred: Cred): Promise<void>;
    writeFile(fname: string, data: any, encoding: string | null, flag: FileFlag, mode: number, cred: Cred, createFile?: boolean): Promise<void>;
    createFile(p: string, flag: FileFlag, mode: number, cred: Cred): Promise<File>;
    stat(path: string, cred: Cred): Promise<Stats>;
    exists(p: string, cred: Cred): Promise<boolean>;
    openFile(path: string, flags: FileFlag, cred: Cred): Promise<File>;
    unlink(path: string, cred: Cred): Promise<void>;
    rmdir(path: string, cred: Cred): Promise<void>;
    mkdir(p: string, mode: any, cred: Cred): Promise<void>;
    readdir(path: string, cred: Cred): Promise<string[]>;
    private newFile;
    private getHandle;
}
export {};
