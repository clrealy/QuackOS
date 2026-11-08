//! FUSE glue: mounts a DuckFS image as a real filesystem you can `ls`, `cp`, `rm`.

use crate::{DuckFs, Inode, Kind};
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};
use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);

pub struct DuckMount {
    fs: DuckFs,
}

fn errno(e: &io::Error) -> i32 {
    e.raw_os_error().unwrap_or(libc::EIO)
}

fn secs(t: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(t)
}

fn to_secs(t: TimeOrNow) -> u64 {
    match t {
        TimeOrNow::Now => SystemTime::now(),
        TimeOrNow::SpecificTime(t) => t,
    }
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0)
}

fn attr(ino: u32, i: &Inode) -> FileAttr {
    FileAttr {
        ino: ino as u64,
        size: i.size,
        blocks: (i.size + 511) / 512,
        atime: secs(i.atime),
        mtime: secs(i.mtime),
        ctime: secs(i.ctime),
        crtime: secs(i.ctime),
        kind: if i.is_dir() { FileType::Directory } else { FileType::RegularFile },
        perm: (i.mode & 0o7777) as u16,
        nlink: i.nlink,
        uid: i.uid,
        gid: i.gid,
        rdev: 0,
        blksize: crate::BS as u32,
        flags: 0,
    }
}

impl DuckMount {
    fn entry(&self, ino: u32, reply: ReplyEntry) {
        match self.fs.read_inode(ino) {
            Ok(i) => reply.entry(&TTL, &attr(ino, &i), 0),
            Err(e) => reply.error(errno(&e)),
        }
    }
}

impl Filesystem for DuckMount {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        match self.fs.lookup(parent as u32, name.as_bytes()) {
            Ok(Some(ino)) => self.entry(ino, reply),
            Ok(None) => reply.error(libc::ENOENT),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        match self.fs.read_inode(ino as u32) {
            Ok(i) => reply.attr(&TTL, &attr(ino as u32, &i)),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let ino = ino as u32;
        if let Some(sz) = size {
            if let Err(e) = self.fs.set_size(ino, sz) {
                return reply.error(errno(&e));
            }
        }
        let res = self.fs.update_inode(ino, |i| {
            if let Some(m) = mode {
                i.mode = (i.mode & !0o7777) | (m & 0o7777);
            }
            if let Some(u) = uid {
                i.uid = u;
            }
            if let Some(g) = gid {
                i.gid = g;
            }
            if let Some(t) = atime {
                i.atime = to_secs(t);
            }
            if let Some(t) = mtime {
                i.mtime = to_secs(t);
            }
        });
        match res {
            Ok(i) => reply.attr(&TTL, &attr(ino, &i)),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn mkdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, mode: u32, umask: u32, reply: ReplyEntry) {
        let perm = mode & !umask & 0o7777;
        match self.fs.create(parent as u32, name.as_bytes(), Kind::Dir, perm, req.uid(), req.gid()) {
            Ok(ino) => self.entry(ino, reply),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn create(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let perm = mode & !umask & 0o7777;
        let res = self
            .fs
            .create(parent as u32, name.as_bytes(), Kind::File, perm, req.uid(), req.gid())
            .and_then(|ino| self.fs.read_inode(ino).map(|i| (ino, i)));
        match res {
            Ok((ino, i)) => reply.created(&TTL, &attr(ino, &i), 0, 0, 0),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        match self.fs.unlink(parent as u32, name.as_bytes()) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        match self.fs.rmdir(parent as u32, name.as_bytes()) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        match self.fs.rename(parent as u32, name.as_bytes(), newparent as u32, newname.as_bytes()) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        match self.fs.read(ino as u32, offset.max(0) as u64, size as usize) {
            Ok(d) => reply.data(&d),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        match self.fs.write(ino as u32, offset.max(0) as u64, data) {
            Ok(n) => reply.written(n as u32),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn fsync(&mut self, _req: &Request<'_>, _ino: u64, _fh: u64, _datasync: bool, reply: ReplyEmpty) {
        match self.fs.sync() {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn readdir(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        let dir = match self.fs.read_inode(ino as u32) {
            Ok(d) => d,
            Err(e) => return reply.error(errno(&e)),
        };
        if !dir.is_dir() {
            return reply.error(libc::ENOTDIR);
        }
        let ents = match self.fs.readdir(ino as u32) {
            Ok(e) => e,
            Err(e) => return reply.error(errno(&e)),
        };
        let mut all: Vec<(u64, FileType, Vec<u8>)> = vec![
            (ino, FileType::Directory, b".".to_vec()),
            (dir.parent as u64, FileType::Directory, b"..".to_vec()),
        ];
        for e in ents {
            let kind = if e.is_dir { FileType::Directory } else { FileType::RegularFile };
            all.push((e.ino as u64, kind, e.name));
        }
        for (idx, (i, kind, name)) in all.into_iter().enumerate().skip(offset.max(0) as usize) {
            if reply.add(i, (idx + 1) as i64, kind, OsStr::from_bytes(&name)) {
                break;
            }
        }
        reply.ok();
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        match self.fs.statfs() {
            Ok(s) => reply.statfs(s.blocks, s.bfree, s.bfree, s.files, s.ffree, s.bsize, crate::MAX_NAME as u32, s.bsize),
            Err(e) => reply.error(errno(&e)),
        }
    }
}

/// Mount `image` at `mountpoint` and block until it is unmounted.
pub fn mount(image: &Path, mountpoint: &Path) -> io::Result<()> {
    let fs = DuckFs::open(image)?;
    let opts = [MountOption::FSName("duckfs".into()), MountOption::DefaultPermissions];
    fuser::mount2(DuckMount { fs }, mountpoint, &opts)
}
