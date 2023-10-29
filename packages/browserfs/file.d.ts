/// <reference types="node" />
import { Stats } from './stats';
export declare enum ActionType {
    NOP = 0,
    THROW_EXCEPTION = 1,
    TRUNCATE_FILE = 2,
    CREATE_FILE = 3
}
/**
 * Represents one of the following file flags. A convenience object.
 *
 * * `'r'` - Open file for reading. An exception occurs if the file does not exist.
 * * `'r+'` - Open file for reading and writing. An exception occurs if the file does not exist.
 * * `'rs'` - Open file for reading in synchronous mode. Instructs the filesystem to not cache writes.
 * * `'rs+'` - Open file for reading and writing, and opens the file in synchronous mode.
 * * `'w'` - Open file for writing. The file is created (if it does not exist) or truncated (if it exists).
 * * `'wx'` - Like 'w' but opens the file in exclusive mode.
 * * `'w+'` - Open file for reading and writing. The file is created (if it does not exist) or truncated (if it exists).
 * * `'wx+'` - Like 'w+' but opens the file in exclusive mode.
 * * `'a'` - Open file for appending. The file is created if it does not exist.
 * * `'ax'` - Like 'a' but opens the file in exclusive mode.
 * * `'a+'` - Open file for reading and appending. The file is created if it does not exist.
 * * `'ax+'` - Like 'a+' but opens the file in exclusive mode.
 *
 * Exclusive mode ensures that the file path is newly created.
 */
export declare class FileFlag {
    private static flagCache;
    private static validFlagStrs;
    /**
     * Get an object representing the given file flag.
     * @param modeStr The string representing the flag
     * @return The FileFlag object representing the flag
     * @throw when the flag string is invalid
     */
    static getFileFlag(flagStr: string): FileFlag;
    private flagStr;
    /**
     * This should never be called directly.
     * @param modeStr The string representing the mode
     * @throw when the mode string is invalid
     */
    constructor(flagStr: string);
    /**
     * Get the underlying flag string for this flag.
     */
    getFlagString(): string;
    /**
     * Get the equivalent mode (0b0xxx: read, write, execute)
     * Note: Execute will always be 0
     */
    getMode(): number;
    /**
     * Returns true if the file is readable.
     */
    isReadable(): boolean;
    /**
     * Returns true if the file is writeable.
     */
    isWriteable(): boolean;
    /**
     * Returns true if the file mode should truncate.
     */
    isTruncating(): boolean;
    /**
     * Returns true if the file is appendable.
     */
    isAppendable(): boolean;
    /**
     * Returns true if the file is open in synchronous mode.
     */
    isSynchronous(): boolean;
    /**
     * Returns true if the file is open in exclusive mode.
     */
    isExclusive(): boolean;
    /**
     * Returns one of the static fields on this object that indicates the
     * appropriate response to the path existing.
     */
    pathExistsAction(): ActionType;
    /**
     * Returns one of the static fields on this object that indicates the
     * appropriate response to the path not existing.
     */
    pathNotExistsAction(): ActionType;
}
export interface File {
    /**
     * **Core**: Get the current file position.
     */
    getPos(): number | undefined;
    /**
     * **Core**: Asynchronous `stat`.
     */
    stat(): Promise<Stats>;
    /**
     * **Core**: Synchronous `stat`.
     */
    statSync(): Stats;
    /**
     * **Core**: Asynchronous close.
     */
    close(): Promise<void>;
    /**
     * **Core**: Synchronous close.
     */
    closeSync(): void;
    /**
     * **Core**: Asynchronous truncate.
     */
    truncate(len: number): Promise<void>;
    /**
     * **Core**: Synchronous truncate.
     */
    truncateSync(len: number): void;
    /**
     * **Core**: Asynchronous sync.
     */
    sync(): Promise<void>;
    /**
     * **Core**: Synchronous sync.
     */
    syncSync(): void;
    /**
     * **Core**: Write buffer to the file.
     * Note that it is unsafe to use fs.write multiple times on the same file
     * without waiting for the callback.
     * @param buffer Buffer containing the data to write to
     *  the file.
     * @param offset Offset in the buffer to start reading data from.
     * @param length The amount of bytes to write to the file.
     * @param position Offset from the beginning of the file where this
     *   data should be written. If position is null, the data will be written at
     *   the current position.
     * @returns Promise resolving to the new length of the buffer
     */
    write(buffer: Buffer, offset: number, length: number, position: number | null): Promise<number>;
    /**
     * **Core**: Write buffer to the file.
     * Note that it is unsafe to use fs.writeSync multiple times on the same file
     * without waiting for it to return.
     * @param buffer Buffer containing the data to write to
     *  the file.
     * @param offset Offset in the buffer to start reading data from.
     * @param length The amount of bytes to write to the file.
     * @param position Offset from the beginning of the file where this
     *   data should be written. If position is null, the data will be written at
     *   the current position.
     */
    writeSync(buffer: Buffer, offset: number, length: number, position: number | null): number;
    /**
     * **Core**: Read data from the file.
     * @param buffer The buffer that the data will be
     *   written to.
     * @param offset The offset within the buffer where writing will
     *   start.
     * @param length An integer specifying the number of bytes to read.
     * @param position An integer specifying where to begin reading from
     *   in the file. If position is null, data will be read from the current file
     *   position.
     * @returns Promise resolving to the new length of the buffer
     */
    read(buffer: Buffer, offset: number, length: number, position: number | null): Promise<{
        bytesRead: number;
        buffer: Buffer;
    }>;
    /**
     * **Core**: Read data from the file.
     * @param buffer The buffer that the data will be written to.
     * @param offset The offset within the buffer where writing will start.
     * @param length An integer specifying the number of bytes to read.
     * @param position An integer specifying where to begin reading from
     *   in the file. If position is null, data will be read from the current file
     *   position.
     */
    readSync(buffer: Buffer, offset: number, length: number, position: number): number;
    /**
     * **Supplementary**: Asynchronous `datasync`.
     *
     * Default implementation maps to `sync`.
     */
    datasync(): Promise<void>;
    /**
     * **Supplementary**: Synchronous `datasync`.
     *
     * Default implementation maps to `syncSync`.
     */
    datasyncSync(): void;
    /**
     * **Optional**: Asynchronous `chown`.
     */
    chown(uid: number, gid: number): Promise<void>;
    /**
     * **Optional**: Synchronous `chown`.
     */
    chownSync(uid: number, gid: number): void;
    /**
     * **Optional**: Asynchronous `fchmod`.
     */
    chmod(mode: number): Promise<void>;
    /**
     * **Optional**: Synchronous `fchmod`.
     */
    chmodSync(mode: number): void;
    /**
     * **Optional**: Change the file timestamps of the file.
     */
    utimes(atime: Date, mtime: Date): Promise<void>;
    /**
     * **Optional**: Change the file timestamps of the file.
     */
    utimesSync(atime: Date, mtime: Date): void;
}
/**
 * Base class that contains shared implementations of functions for the file
 * object.
 */
export declare class BaseFile {
    sync(): Promise<void>;
    syncSync(): void;
    datasync(): Promise<void>;
    datasyncSync(): void;
    chown(uid: number, gid: number): Promise<void>;
    chownSync(uid: number, gid: number): void;
    chmod(mode: number): Promise<void>;
    chmodSync(mode: number): void;
    utimes(atime: Date, mtime: Date): Promise<void>;
    utimesSync(atime: Date, mtime: Date): void;
}
