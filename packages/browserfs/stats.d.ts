/// <reference types="node" />
/// <reference types="node" />
import type { StatsBase } from 'fs';
import { Cred } from './cred';
/**
 * Indicates the type of the given file. Applied to 'mode'.
 */
export declare enum FileType {
    FILE,
    DIRECTORY,
    SYMLINK
}
/**
 * Implementation of Node's `Stats`.
 *
 * Attribute descriptions are from `man 2 stat'
 * @see http://nodejs.org/api/fs.html#fs_class_fs_stats
 * @see http://man7.org/linux/man-pages/man2/stat.2.html
 */
export declare class Stats implements StatsBase<number> {
    static fromBuffer(buffer: Buffer): Stats;
    /**
     * Clones the stats object.
     */
    static clone(s: Stats): Stats;
    blocks: number;
    mode: number;
    dev: number;
    ino: number;
    rdev: number;
    nlink: number;
    blksize: number;
    uid: number;
    gid: number;
    fileData: Buffer | null;
    atimeMs: number;
    mtimeMs: number;
    ctimeMs: number;
    birthtimeMs: number;
    size: number;
    get atime(): Date;
    get mtime(): Date;
    get ctime(): Date;
    get birthtime(): Date;
    /**
     * Provides information about a particular entry in the file system.
     * @param itemType Type of the item (FILE, DIRECTORY, SYMLINK, or SOCKET)
     * @param size Size of the item in bytes. For directories/symlinks,
     *   this is normally the size of the struct that represents the item.
     * @param mode Unix-style file mode (e.g. 0o644)
     * @param atimeMs time of last access, in milliseconds since epoch
     * @param mtimeMs time of last modification, in milliseconds since epoch
     * @param ctimeMs time of last time file status was changed, in milliseconds since epoch
     * @param uid the id of the user that owns the file
     * @param gid the id of the group that owns the file
     * @param birthtimeMs time of file creation, in milliseconds since epoch
     */
    constructor(itemType: FileType, size: number, mode?: number, atimeMs?: number, mtimeMs?: number, ctimeMs?: number, uid?: number, gid?: number, birthtimeMs?: number);
    toBuffer(): Buffer;
    /**
     * @return [Boolean] True if this item is a file.
     */
    isFile(): boolean;
    /**
     * @return [Boolean] True if this item is a directory.
     */
    isDirectory(): boolean;
    /**
     * @return [Boolean] True if this item is a symbolic link (only valid through lstat)
     */
    isSymbolicLink(): boolean;
    /**
     * Checks if a given user/group has access to this item
     * @param mode The request access as 4 bits (unused, read, write, execute)
     * @param uid The requesting UID
     * @param gid The requesting GID
     * @returns [Boolean] True if the request has access, false if the request does not
     */
    hasAccess(mode: number, cred: Cred): boolean;
    /**
     * Convert the current stats object into a cred object
     */
    getCred(uid?: number, gid?: number): Cred;
    /**
     * Change the mode of the file. We use this helper function to prevent messing
     * up the type of the file, which is encoded in mode.
     */
    chmod(mode: number): void;
    /**
     * Change the owner user/group of the file.
     * This function makes sure it is a valid UID/GID (that is, a 32 unsigned int)
     */
    chown(uid: number, gid: number): void;
    isSocket(): boolean;
    isBlockDevice(): boolean;
    isCharacterDevice(): boolean;
    isFIFO(): boolean;
}
