# DuckFS 🦆

The filesystem for QuackOS. Real, block-based, written in Rust, mounts through FUSE.

## Build
    cargo build --release

## Use
    duckfs mkfs  quack.img 64        # make a 64 MiB image
    duckfs mount quack.img /mnt/duck # mount (needs FUSE, runs in foreground)
    fusermount -u /mnt/duck          # unmount
    duckfs info  quack.img           # geometry + free space

## Test
    cargo test

## Layout (4 KiB blocks)
superblock | inode bitmap | data bitmap | inode table | data blocks

- 128-byte inodes, 12 direct + 1 indirect + 1 double-indirect pointers (max file ~4 GiB)
- directories are files of 64-byte entries (names up to 59 bytes)
- supports: create, read, write, truncate, mkdir, rmdir, unlink, rename, chmod/chown/utimens, statfs, sparse files

## Not done yet
symlinks, hard links, xattrs, journaling/crash safety, permissions beyond the kernel's DefaultPermissions
