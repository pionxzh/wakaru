/**
 * Credentials used for FS ops.
 * Similar to Linux's cred struct. See https://github.com/torvalds/linux/blob/master/include/linux/cred.h
 */
export declare class Cred {
    uid: number;
    gid: number;
    suid: number;
    sgid: number;
    euid: number;
    egid: number;
    constructor(uid: number, gid: number, suid: number, sgid: number, euid: number, egid: number);
    static Root: Cred;
}
