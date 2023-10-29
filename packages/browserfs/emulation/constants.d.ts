/** Constant for fs.access(). File is visible to the calling process. */
export declare const F_OK = 0;
/** Constant for fs.access(). File can be read by the calling process. */
export declare const R_OK = 4;
/** Constant for fs.access(). File can be written by the calling process. */
export declare const W_OK = 2;
/** Constant for fs.access(). File can be executed by the calling process. */
export declare const X_OK = 1;
/** Constant for fs.copyFile. Flag indicating the destination file should not be overwritten if it already exists. */
export declare const COPYFILE_EXCL = 1;
/**
 * Constant for fs.copyFile. Copy operation will attempt to create a copy-on-write reflink.
 * If the underlying platform does not support copy-on-write, then a fallback copy mechanism is used.
 */
export declare const COPYFILE_FICLONE = 2;
/**
 * Constant for fs.copyFile. Copy operation will attempt to create a copy-on-write reflink.
 * If the underlying platform does not support copy-on-write, then the operation will fail with an error.
 */
export declare const COPYFILE_FICLONE_FORCE = 4;
/** Constant for fs.open(). Flag indicating to open a file for read-only access. */
export declare const O_RDONLY = 0;
/** Constant for fs.open(). Flag indicating to open a file for write-only access. */
export declare const O_WRONLY = 1;
/** Constant for fs.open(). Flag indicating to open a file for read-write access. */
export declare const O_RDWR = 2;
/** Constant for fs.open(). Flag indicating to create the file if it does not already exist. */
export declare const O_CREAT = 64;
/** Constant for fs.open(). Flag indicating that opening a file should fail if the O_CREAT flag is set and the file already exists. */
export declare const O_EXCL = 128;
/**
 * Constant for fs.open(). Flag indicating that if path identifies a terminal device,
 * opening the path shall not cause that terminal to become the controlling terminal for the process
 * (if the process does not already have one).
 */
export declare const O_NOCTTY = 256;
/** Constant for fs.open(). Flag indicating that if the file exists and is a regular file, and the file is opened successfully for write access, its length shall be truncated to zero. */
export declare const O_TRUNC = 512;
/** Constant for fs.open(). Flag indicating that data will be appended to the end of the file. */
export declare const O_APPEND = 1024;
/** Constant for fs.open(). Flag indicating that the open should fail if the path is not a directory. */
export declare const O_DIRECTORY = 65536;
/**
 * constant for fs.open().
 * Flag indicating reading accesses to the file system will no longer result in
 * an update to the atime information associated with the file.
 * This flag is available on Linux operating systems only.
 */
export declare const O_NOATIME = 262144;
/** Constant for fs.open(). Flag indicating that the open should fail if the path is a symbolic link. */
export declare const O_NOFOLLOW = 131072;
/** Constant for fs.open(). Flag indicating that the file is opened for synchronous I/O. */
export declare const O_SYNC = 1052672;
/** Constant for fs.open(). Flag indicating that the file is opened for synchronous I/O with write operations waiting for data integrity. */
export declare const O_DSYNC = 4096;
/** Constant for fs.open(). Flag indicating to open the symbolic link itself rather than the resource it is pointing to. */
export declare const O_SYMLINK = 32768;
/** Constant for fs.open(). When set, an attempt will be made to minimize caching effects of file I/O. */
export declare const O_DIRECT = 16384;
/** Constant for fs.open(). Flag indicating to open the file in nonblocking mode when possible. */
export declare const O_NONBLOCK = 2048;
/** Constant for fs.Stats mode property for determining a file's type. Bit mask used to extract the file type code. */
export declare const S_IFMT = 61440;
/** Constant for fs.Stats mode property for determining a file's type. File type constant for a regular file. */
export declare const S_IFREG = 32768;
/** Constant for fs.Stats mode property for determining a file's type. File type constant for a directory. */
export declare const S_IFDIR = 16384;
/** Constant for fs.Stats mode property for determining a file's type. File type constant for a character-oriented device file. */
export declare const S_IFCHR = 8192;
/** Constant for fs.Stats mode property for determining a file's type. File type constant for a block-oriented device file. */
export declare const S_IFBLK = 24576;
/** Constant for fs.Stats mode property for determining a file's type. File type constant for a FIFO/pipe. */
export declare const S_IFIFO = 4096;
/** Constant for fs.Stats mode property for determining a file's type. File type constant for a symbolic link. */
export declare const S_IFLNK = 40960;
/** Constant for fs.Stats mode property for determining a file's type. File type constant for a socket. */
export declare const S_IFSOCK = 49152;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating readable, writable and executable by owner. */
export declare const S_IRWXU = 448;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating readable by owner. */
export declare const S_IRUSR = 256;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating writable by owner. */
export declare const S_IWUSR = 128;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating executable by owner. */
export declare const S_IXUSR = 64;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating readable, writable and executable by group. */
export declare const S_IRWXG = 56;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating readable by group. */
export declare const S_IRGRP = 32;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating writable by group. */
export declare const S_IWGRP = 16;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating executable by group. */
export declare const S_IXGRP = 8;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating readable, writable and executable by others. */
export declare const S_IRWXO = 7;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating readable by others. */
export declare const S_IROTH = 4;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating writable by others. */
export declare const S_IWOTH = 2;
/** Constant for fs.Stats mode property for determining access permissions for a file. File mode indicating executable by others. */
export declare const S_IXOTH = 1;
