//! DuckFS core: a small, real, block-based filesystem that lives inside a
//! single image file (or any file / block device you can open read+write).
//!
//! On-disk layout (4 KiB blocks, all integers little-endian):
//!
//!   block 0            superblock (magic "DUCKFS01" + geometry)
//!   block 1            inode bitmap  (1 bit per inode)
//!   blocks 2..         data bitmap   (1 bit per block, metadata blocks pre-marked used)
//!   next               inode table   (128-byte inodes, 32 per block)
//!   rest               data blocks
//!
//! Inodes have 12 direct pointers, 1 indirect and 1 double-indirect block,
//! so a single file can reach ~4 GiB. Directories are plain files made of
//! fixed 64-byte entries (u32 inode, u8 name length, up to 59 name bytes).
//!
//! No journal yet: a crash mid-operation can leave the image inconsistent.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod mount;

pub const BS: usize = 4096;
pub const MAGIC: &[u8; 8] = b"DUCKFS01";
pub const INODE_SIZE: usize = 128;
pub const NDIRECT: usize = 12;
pub const PTRS: usize = BS / 4;
pub const DIRENT_SIZE: usize = 64;
pub const MAX_NAME: usize = 59;
pub const ROOT_INO: u32 = 1;
pub const MAX_FILE_SIZE: u64 = ((NDIRECT + PTRS + PTRS * PTRS) * BS) as u64;
const INODES_PER_BLOCK: usize = BS / INODE_SIZE;

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

fn err(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn get32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn get64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inode {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub size: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub direct: [u32; NDIRECT],
    pub indirect: u32,
    pub dindirect: u32,
    pub parent: u32,
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }

    fn encode(&self, b: &mut [u8]) {
        put32(b, 0, self.mode);
        put32(b, 4, self.uid);
        put32(b, 8, self.gid);
        put32(b, 12, self.nlink);
        put64(b, 16, self.size);
        put64(b, 24, self.atime);
        put64(b, 32, self.mtime);
        put64(b, 40, self.ctime);
        for i in 0..NDIRECT {
            put32(b, 48 + i * 4, self.direct[i]);
        }
        put32(b, 96, self.indirect);
        put32(b, 100, self.dindirect);
        put32(b, 104, self.parent);
    }

    fn decode(b: &[u8]) -> Inode {
        let mut direct = [0u32; NDIRECT];
        for i in 0..NDIRECT {
            direct[i] = get32(b, 48 + i * 4);
        }
        Inode {
            mode: get32(b, 0),
            uid: get32(b, 4),
            gid: get32(b, 8),
            nlink: get32(b, 12),
            size: get64(b, 16),
            atime: get64(b, 24),
            mtime: get64(b, 32),
            ctime: get64(b, 40),
            direct,
            indirect: get32(b, 96),
            dindirect: get32(b, 100),
            parent: get32(b, 104),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Super {
    pub total_blocks: u32,
    pub inode_count: u32,
    pub ibm_start: u32,
    pub dbm_start: u32,
    pub dbm_blocks: u32,
    pub itable_start: u32,
    pub itable_blocks: u32,
    pub data_start: u32,
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub ino: u32,
    pub name: Vec<u8>,
    pub is_dir: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct StatFs {
    pub blocks: u64,
    pub bfree: u64,
    pub files: u64,
    pub ffree: u64,
    pub bsize: u32,
}

pub struct DuckFs {
    file: File,
    pub sb: Super,
}

impl DuckFs {
    // ---------------------------------------------------------------- mkfs / open

    /// Create a fresh DuckFS image of `size_bytes` at `path` (overwrites it).
    pub fn format(path: &Path, size_bytes: u64) -> io::Result<()> {
        let total_blocks = (size_bytes / BS as u64) as u32;
        if total_blocks < 64 {
            return Err(err(libc::EINVAL));
        }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
        file.set_len(total_blocks as u64 * BS as u64)?;

        let mut inode_count = (total_blocks / 4).clamp(32, 32768);
        inode_count -= inode_count % INODES_PER_BLOCK as u32;
        let dbm_blocks = (total_blocks as usize + BS * 8 - 1) / (BS * 8);
        let itable_blocks = inode_count / INODES_PER_BLOCK as u32;
        let ibm_start = 1;
        let dbm_start = 2;
        let itable_start = dbm_start + dbm_blocks as u32;
        let data_start = itable_start + itable_blocks;
        if data_start + 8 > total_blocks {
            return Err(err(libc::EINVAL));
        }
        let sb = Super {
            total_blocks,
            inode_count,
            ibm_start,
            dbm_start,
            dbm_blocks: dbm_blocks as u32,
            itable_start,
            itable_blocks,
            data_start,
        };
        let fs = DuckFs { file, sb };

        let mut blk = vec![0u8; BS];
        blk[0..8].copy_from_slice(MAGIC);
        put32(&mut blk, 8, BS as u32);
        put32(&mut blk, 12, sb.total_blocks);
        put32(&mut blk, 16, sb.inode_count);
        put32(&mut blk, 20, sb.ibm_start);
        put32(&mut blk, 24, sb.dbm_start);
        put32(&mut blk, 28, sb.dbm_blocks);
        put32(&mut blk, 32, sb.itable_start);
        put32(&mut blk, 36, sb.itable_blocks);
        put32(&mut blk, 40, sb.data_start);
        fs.write_block(0, &blk)?;

        for b in 0..data_start {
            fs.bit_set(sb.dbm_start, b)?;
        }

        let root = fs.alloc_inode()?;
        debug_assert_eq!(root, ROOT_INO);
        let t = now();
        let ino = Inode {
            mode: S_IFDIR | 0o755,
            nlink: 2,
            atime: t,
            mtime: t,
            ctime: t,
            parent: ROOT_INO,
            ..Default::default()
        };
        fs.write_inode(ROOT_INO, &ino)?;
        fs.file.sync_all()
    }

    pub fn open(path: &Path) -> io::Result<DuckFs> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut blk = vec![0u8; BS];
        file.read_exact_at(&mut blk, 0)?;
        if &blk[0..8] != MAGIC || get32(&blk, 8) as usize != BS {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a DuckFS image"));
        }
        let sb = Super {
            total_blocks: get32(&blk, 12),
            inode_count: get32(&blk, 16),
            ibm_start: get32(&blk, 20),
            dbm_start: get32(&blk, 24),
            dbm_blocks: get32(&blk, 28),
            itable_start: get32(&blk, 32),
            itable_blocks: get32(&blk, 36),
            data_start: get32(&blk, 40),
        };
        Ok(DuckFs { file, sb })
    }

    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_all()
    }

    // ---------------------------------------------------------------- raw block io

    fn read_block(&self, b: u32) -> io::Result<Vec<u8>> {
        if b >= self.sb.total_blocks {
            return Err(err(libc::EIO));
        }
        let mut buf = vec![0u8; BS];
        self.file.read_exact_at(&mut buf, b as u64 * BS as u64)?;
        Ok(buf)
    }

    fn write_block(&self, b: u32, buf: &[u8]) -> io::Result<()> {
        if b >= self.sb.total_blocks || buf.len() != BS {
            return Err(err(libc::EIO));
        }
        self.file.write_all_at(buf, b as u64 * BS as u64)
    }

    // ---------------------------------------------------------------- bitmaps

    fn bit_set(&self, start: u32, idx: u32) -> io::Result<()> {
        let per = (BS * 8) as u32;
        let blk_no = start + idx / per;
        let bit = (idx % per) as usize;
        let mut blk = self.read_block(blk_no)?;
        blk[bit / 8] |= 1 << (bit % 8);
        self.write_block(blk_no, &blk)
    }

    fn bit_clear(&self, start: u32, idx: u32) -> io::Result<()> {
        let per = (BS * 8) as u32;
        let blk_no = start + idx / per;
        let bit = (idx % per) as usize;
        let mut blk = self.read_block(blk_no)?;
        blk[bit / 8] &= !(1 << (bit % 8));
        self.write_block(blk_no, &blk)
    }

    fn bit_alloc(&self, start: u32, nbits: u32) -> io::Result<Option<u32>> {
        let nblocks = (nbits as usize + BS * 8 - 1) / (BS * 8);
        for bi in 0..nblocks {
            let mut blk = self.read_block(start + bi as u32)?;
            for byte in 0..BS {
                if blk[byte] == 0xFF {
                    continue;
                }
                for bit in 0..8 {
                    let idx = ((bi * BS + byte) * 8 + bit) as u32;
                    if idx >= nbits {
                        return Ok(None);
                    }
                    if blk[byte] & (1 << bit) == 0 {
                        blk[byte] |= 1 << bit;
                        self.write_block(start + bi as u32, &blk)?;
                        return Ok(Some(idx));
                    }
                }
            }
        }
        Ok(None)
    }

    fn bit_count_free(&self, start: u32, nbits: u32) -> io::Result<u32> {
        let nblocks = (nbits as usize + BS * 8 - 1) / (BS * 8);
        let mut free = 0u32;
        for bi in 0..nblocks {
            let blk = self.read_block(start + bi as u32)?;
            for bit_idx in 0..(BS * 8) {
                let idx = (bi * BS * 8 + bit_idx) as u32;
                if idx >= nbits {
                    break;
                }
                if blk[bit_idx / 8] & (1 << (bit_idx % 8)) == 0 {
                    free += 1;
                }
            }
        }
        Ok(free)
    }

    fn alloc_block(&self) -> io::Result<u32> {
        match self.bit_alloc(self.sb.dbm_start, self.sb.total_blocks)? {
            Some(b) => {
                self.write_block(b, &vec![0u8; BS])?;
                Ok(b)
            }
            None => Err(err(libc::ENOSPC)),
        }
    }

    fn free_block(&self, b: u32) -> io::Result<()> {
        if b < self.sb.data_start || b >= self.sb.total_blocks {
            return Err(err(libc::EIO));
        }
        self.bit_clear(self.sb.dbm_start, b)
    }

    fn alloc_inode(&self) -> io::Result<u32> {
        match self.bit_alloc(self.sb.ibm_start, self.sb.inode_count)? {
            Some(i) => Ok(i + 1),
            None => Err(err(libc::ENOSPC)),
        }
    }

    fn free_inode(&self, ino: u32) -> io::Result<()> {
        self.write_inode(ino, &Inode::default())?;
        self.bit_clear(self.sb.ibm_start, ino - 1)
    }

    // ---------------------------------------------------------------- inodes

    fn inode_loc(&self, ino: u32) -> io::Result<(u32, usize)> {
        if ino == 0 || ino > self.sb.inode_count {
            return Err(err(libc::ENOENT));
        }
        let i = (ino - 1) as usize;
        Ok((
            self.sb.itable_start + (i / INODES_PER_BLOCK) as u32,
            (i % INODES_PER_BLOCK) * INODE_SIZE,
        ))
    }

    pub fn read_inode(&self, ino: u32) -> io::Result<Inode> {
        let (b, o) = self.inode_loc(ino)?;
        let blk = self.read_block(b)?;
        let inode = Inode::decode(&blk[o..o + INODE_SIZE]);
        if inode.mode == 0 {
            return Err(err(libc::ENOENT));
        }
        Ok(inode)
    }

    fn write_inode(&self, ino: u32, inode: &Inode) -> io::Result<()> {
        let (b, o) = self.inode_loc(ino)?;
        let mut blk = self.read_block(b)?;
        inode.encode(&mut blk[o..o + INODE_SIZE]);
        self.write_block(b, &blk)
    }

    /// Read-modify-write helper for chmod / chown / utimens style changes.
    pub fn update_inode<F: FnOnce(&mut Inode)>(&self, ino: u32, f: F) -> io::Result<Inode> {
        let mut inode = self.read_inode(ino)?;
        f(&mut inode);
        inode.ctime = now();
        self.write_inode(ino, &inode)?;
        Ok(inode)
    }

    // ---------------------------------------------------------------- block mapping

    fn root_slot(&self, slot: &mut u32, alloc: bool) -> io::Result<Option<u32>> {
        if *slot == 0 {
            if !alloc {
                return Ok(None);
            }
            *slot = self.alloc_block()?;
        }
        Ok(Some(*slot))
    }

    fn ptr_slot(&self, table: u32, idx: usize, alloc: bool) -> io::Result<Option<u32>> {
        let mut t = self.read_block(table)?;
        let p = get32(&t, idx * 4);
        if p != 0 {
            return Ok(Some(p));
        }
        if !alloc {
            return Ok(None);
        }
        let b = self.alloc_block()?;
        put32(&mut t, idx * 4, b);
        self.write_block(table, &t)?;
        Ok(Some(b))
    }

    /// Map logical block `lbn` of `inode` to a disk block (allocating if asked).
    fn bmap(&self, inode: &mut Inode, lbn: usize, alloc: bool) -> io::Result<Option<u32>> {
        if lbn < NDIRECT {
            return self.root_slot(&mut inode.direct[lbn], alloc);
        }
        let l = lbn - NDIRECT;
        if l < PTRS {
            let ind = match self.root_slot(&mut inode.indirect, alloc)? {
                Some(b) => b,
                None => return Ok(None),
            };
            return self.ptr_slot(ind, l, alloc);
        }
        let l = l - PTRS;
        if l < PTRS * PTRS {
            let d = match self.root_slot(&mut inode.dindirect, alloc)? {
                Some(b) => b,
                None => return Ok(None),
            };
            let ind = match self.ptr_slot(d, l / PTRS, alloc)? {
                Some(b) => b,
                None => return Ok(None),
            };
            return self.ptr_slot(ind, l % PTRS, alloc);
        }
        Err(err(libc::EFBIG))
    }

    /// Free one logical block (data only; index blocks are freed on full release).
    fn bmap_free(&self, inode: &mut Inode, lbn: usize) -> io::Result<()> {
        if lbn < NDIRECT {
            if inode.direct[lbn] != 0 {
                self.free_block(inode.direct[lbn])?;
                inode.direct[lbn] = 0;
            }
            return Ok(());
        }
        let l = lbn - NDIRECT;
        if l < PTRS {
            if inode.indirect != 0 {
                self.clear_ptr(inode.indirect, l)?;
            }
            return Ok(());
        }
        let l = l - PTRS;
        if l < PTRS * PTRS && inode.dindirect != 0 {
            let d = self.read_block(inode.dindirect)?;
            let ind = get32(&d, (l / PTRS) * 4);
            if ind != 0 {
                self.clear_ptr(ind, l % PTRS)?;
            }
        }
        Ok(())
    }

    fn clear_ptr(&self, table: u32, idx: usize) -> io::Result<()> {
        let mut t = self.read_block(table)?;
        let p = get32(&t, idx * 4);
        if p != 0 {
            self.free_block(p)?;
            put32(&mut t, idx * 4, 0);
            self.write_block(table, &t)?;
        }
        Ok(())
    }

    fn free_table(&self, table: u32, depth: u32) -> io::Result<()> {
        let t = self.read_block(table)?;
        for i in 0..PTRS {
            let p = get32(&t, i * 4);
            if p != 0 {
                if depth > 1 {
                    self.free_table(p, depth - 1)?;
                } else {
                    self.free_block(p)?;
                }
            }
        }
        self.free_block(table)
    }

    /// Free every block an inode owns (data + index blocks).
    fn free_all_blocks(&self, inode: &mut Inode) -> io::Result<()> {
        for i in 0..NDIRECT {
            if inode.direct[i] != 0 {
                self.free_block(inode.direct[i])?;
                inode.direct[i] = 0;
            }
        }
        if inode.indirect != 0 {
            self.free_table(inode.indirect, 1)?;
            inode.indirect = 0;
        }
        if inode.dindirect != 0 {
            self.free_table(inode.dindirect, 2)?;
            inode.dindirect = 0;
        }
        Ok(())
    }

    fn truncate_blocks(&self, inode: &mut Inode, new_size: u64) -> io::Result<()> {
        if new_size == 0 {
            return self.free_all_blocks(inode);
        }
        let keep = ((new_size + BS as u64 - 1) / BS as u64) as usize;
        let old = ((inode.size + BS as u64 - 1) / BS as u64) as usize;
        for lbn in keep..old {
            self.bmap_free(inode, lbn)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- file data

    pub fn read(&self, ino: u32, off: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut inode = self.read_inode(ino)?;
        if off >= inode.size {
            return Ok(Vec::new());
        }
        let end = (off + len as u64).min(inode.size);
        let mut out = Vec::with_capacity((end - off) as usize);
        let mut pos = off;
        while pos < end {
            let lbn = (pos / BS as u64) as usize;
            let boff = (pos % BS as u64) as usize;
            let n = (BS - boff).min((end - pos) as usize);
            match self.bmap(&mut inode, lbn, false)? {
                Some(b) => {
                    let blk = self.read_block(b)?;
                    out.extend_from_slice(&blk[boff..boff + n]);
                }
                None => out.resize(out.len() + n, 0),
            }
            pos += n as u64;
        }
        Ok(out)
    }

    /// Write `data` at `off`. Returns how many bytes were written (short on ENOSPC).
    pub fn write(&self, ino: u32, off: u64, data: &[u8]) -> io::Result<usize> {
        let mut inode = self.read_inode(ino)?;
        if off + data.len() as u64 > MAX_FILE_SIZE {
            return Err(err(libc::EFBIG));
        }
        let mut done = 0usize;
        let mut failure: Option<io::Error> = None;
        while done < data.len() {
            let pos = off + done as u64;
            let lbn = (pos / BS as u64) as usize;
            let boff = (pos % BS as u64) as usize;
            let n = (BS - boff).min(data.len() - done);
            let b = match self.bmap(&mut inode, lbn, true) {
                Ok(Some(b)) => b,
                Ok(None) => unreachable!(),
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            };
            let mut blk = if n == BS { vec![0u8; BS] } else { self.read_block(b)? };
            blk[boff..boff + n].copy_from_slice(&data[done..done + n]);
            self.write_block(b, &blk)?;
            done += n;
        }
        let end = off + done as u64;
        if end > inode.size {
            inode.size = end;
        }
        let t = now();
        inode.mtime = t;
        inode.ctime = t;
        self.write_inode(ino, &inode)?;
        match failure {
            Some(e) if done == 0 => Err(e),
            _ => Ok(done),
        }
    }

    pub fn set_size(&self, ino: u32, size: u64) -> io::Result<()> {
        let mut inode = self.read_inode(ino)?;
        if inode.is_dir() {
            return Err(err(libc::EISDIR));
        }
        if size > MAX_FILE_SIZE {
            return Err(err(libc::EFBIG));
        }
        if size < inode.size {
            self.truncate_blocks(&mut inode, size)?;
            let boff = (size % BS as u64) as usize;
            if boff != 0 {
                if let Some(b) = self.bmap(&mut inode, (size / BS as u64) as usize, false)? {
                    let mut blk = self.read_block(b)?;
                    for x in &mut blk[boff..] {
                        *x = 0;
                    }
                    self.write_block(b, &blk)?;
                }
            }
        }
        inode.size = size;
        let t = now();
        inode.mtime = t;
        inode.ctime = t;
        self.write_inode(ino, &inode)
    }

    // ---------------------------------------------------------------- directories

    fn check_name(name: &[u8]) -> io::Result<()> {
        if name.is_empty() {
            return Err(err(libc::EINVAL));
        }
        if name.len() > MAX_NAME {
            return Err(err(libc::ENAMETOOLONG));
        }
        if name == b"." || name == b".." {
            return Err(err(libc::EEXIST));
        }
        Ok(())
    }

    fn dir_slots(&self, dir: u32) -> io::Result<Vec<(usize, u32, Vec<u8>)>> {
        let inode = self.read_inode(dir)?;
        if !inode.is_dir() {
            return Err(err(libc::ENOTDIR));
        }
        let data = self.read(dir, 0, inode.size as usize)?;
        let mut out = Vec::new();
        for (slot, chunk) in data.chunks_exact(DIRENT_SIZE).enumerate() {
            let i = get32(chunk, 0);
            if i != 0 {
                let n = chunk[4] as usize;
                out.push((slot, i, chunk[5..5 + n].to_vec()));
            }
        }
        Ok(out)
    }

    fn dir_add(&self, dir: u32, name: &[u8], ino: u32) -> io::Result<()> {
        let inode = self.read_inode(dir)?;
        let used: Vec<usize> = self.dir_slots(dir)?.into_iter().map(|(s, _, _)| s).collect();
        let total = (inode.size as usize) / DIRENT_SIZE;
        let slot = (0..total).find(|s| !used.contains(s)).unwrap_or(total);
        let mut ent = vec![0u8; DIRENT_SIZE];
        put32(&mut ent, 0, ino);
        ent[4] = name.len() as u8;
        ent[5..5 + name.len()].copy_from_slice(name);
        let n = self.write(dir, (slot * DIRENT_SIZE) as u64, &ent)?;
        if n != DIRENT_SIZE {
            return Err(err(libc::ENOSPC));
        }
        Ok(())
    }

    fn dir_remove(&self, dir: u32, name: &[u8]) -> io::Result<u32> {
        for (slot, ino, n) in self.dir_slots(dir)? {
            if n == name {
                self.write(dir, (slot * DIRENT_SIZE) as u64, &vec![0u8; DIRENT_SIZE])?;
                return Ok(ino);
            }
        }
        Err(err(libc::ENOENT))
    }

    pub fn lookup(&self, parent: u32, name: &[u8]) -> io::Result<Option<u32>> {
        let pin = self.read_inode(parent)?;
        if !pin.is_dir() {
            return Err(err(libc::ENOTDIR));
        }
        if name == b"." {
            return Ok(Some(parent));
        }
        if name == b".." {
            return Ok(Some(pin.parent));
        }
        for (_, ino, n) in self.dir_slots(parent)? {
            if n == name {
                return Ok(Some(ino));
            }
        }
        Ok(None)
    }

    pub fn readdir(&self, dir: u32) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for (_, ino, name) in self.dir_slots(dir)? {
            let is_dir = self.read_inode(ino)?.is_dir();
            out.push(DirEntry { ino, name, is_dir });
        }
        Ok(out)
    }

    fn touch_dir(&self, dir: u32, nlink_delta: i32) -> io::Result<()> {
        let mut p = self.read_inode(dir)?;
        p.nlink = (p.nlink as i64 + nlink_delta as i64).max(0) as u32;
        let t = now();
        p.mtime = t;
        p.ctime = t;
        self.write_inode(dir, &p)
    }

    pub fn create(&self, parent: u32, name: &[u8], kind: Kind, perm: u32, uid: u32, gid: u32) -> io::Result<u32> {
        Self::check_name(name)?;
        if !self.read_inode(parent)?.is_dir() {
            return Err(err(libc::ENOTDIR));
        }
        if self.lookup(parent, name)?.is_some() {
            return Err(err(libc::EEXIST));
        }
        let ino = self.alloc_inode()?;
        let t = now();
        let (mode, nlink) = match kind {
            Kind::Dir => (S_IFDIR | (perm & 0o7777), 2),
            Kind::File => (S_IFREG | (perm & 0o7777), 1),
        };
        let inode = Inode { mode, uid, gid, nlink, atime: t, mtime: t, ctime: t, parent, ..Default::default() };
        self.write_inode(ino, &inode)?;
        if let Err(e) = self.dir_add(parent, name, ino) {
            self.free_inode(ino)?;
            return Err(e);
        }
        self.touch_dir(parent, if kind == Kind::Dir { 1 } else { 0 })?;
        Ok(ino)
    }

    pub fn unlink(&self, parent: u32, name: &[u8]) -> io::Result<()> {
        let ino = self.lookup(parent, name)?.ok_or_else(|| err(libc::ENOENT))?;
        if name == b"." || name == b".." || self.read_inode(ino)?.is_dir() {
            return Err(err(libc::EISDIR));
        }
        self.dir_remove(parent, name)?;
        let mut inode = self.read_inode(ino)?;
        self.free_all_blocks(&mut inode)?;
        self.free_inode(ino)?;
        self.touch_dir(parent, 0)
    }

    pub fn rmdir(&self, parent: u32, name: &[u8]) -> io::Result<()> {
        if name == b"." {
            return Err(err(libc::EINVAL));
        }
        if name == b".." {
            return Err(err(libc::ENOTEMPTY));
        }
        let ino = self.lookup(parent, name)?.ok_or_else(|| err(libc::ENOENT))?;
        let mut inode = self.read_inode(ino)?;
        if !inode.is_dir() {
            return Err(err(libc::ENOTDIR));
        }
        if !self.dir_slots(ino)?.is_empty() {
            return Err(err(libc::ENOTEMPTY));
        }
        self.dir_remove(parent, name)?;
        self.free_all_blocks(&mut inode)?;
        self.free_inode(ino)?;
        self.touch_dir(parent, -1)
    }

    pub fn rename(&self, old_parent: u32, old_name: &[u8], new_parent: u32, new_name: &[u8]) -> io::Result<()> {
        if old_name == b"." || old_name == b".." {
            return Err(err(libc::EINVAL));
        }
        Self::check_name(new_name)?;
        let src = self.lookup(old_parent, old_name)?.ok_or_else(|| err(libc::ENOENT))?;
        let sin = self.read_inode(src)?;
        if !self.read_inode(new_parent)?.is_dir() {
            return Err(err(libc::ENOTDIR));
        }
        // can't move a directory into itself / its own subtree
        if sin.is_dir() {
            let mut cur = new_parent;
            loop {
                if cur == src {
                    return Err(err(libc::EINVAL));
                }
                if cur == ROOT_INO {
                    break;
                }
                cur = self.read_inode(cur)?.parent;
            }
        }
        if let Some(dst) = self.lookup(new_parent, new_name)? {
            if dst == src {
                return Ok(());
            }
            let din = self.read_inode(dst)?;
            match (sin.is_dir(), din.is_dir()) {
                (true, true) => self.rmdir(new_parent, new_name)?,
                (true, false) => return Err(err(libc::ENOTDIR)),
                (false, true) => return Err(err(libc::EISDIR)),
                (false, false) => self.unlink(new_parent, new_name)?,
            }
        }
        self.dir_add(new_parent, new_name, src)?;
        self.dir_remove(old_parent, old_name)?;
        if sin.is_dir() && old_parent != new_parent {
            self.update_inode(src, |i| i.parent = new_parent)?;
            self.touch_dir(old_parent, -1)?;
            self.touch_dir(new_parent, 1)?;
        } else {
            self.touch_dir(old_parent, 0)?;
            if old_parent != new_parent {
                self.touch_dir(new_parent, 0)?;
            }
        }
        Ok(())
    }

    pub fn statfs(&self) -> io::Result<StatFs> {
        Ok(StatFs {
            blocks: self.sb.total_blocks as u64,
            bfree: self.bit_count_free(self.sb.dbm_start, self.sb.total_blocks)? as u64,
            files: self.sb.inode_count as u64,
            ffree: self.bit_count_free(self.sb.ibm_start, self.sb.inode_count)? as u64,
            bsize: BS as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn fresh(mb: u64) -> (Tmp, DuckFs) {
        let p = std::env::temp_dir().join(format!("duckfs-test-{}-{}.img", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        DuckFs::format(&p, mb * 1024 * 1024).unwrap();
        let fs = DuckFs::open(&p).unwrap();
        (Tmp(p), fs)
    }

    fn errno(r: io::Result<impl std::fmt::Debug>) -> i32 {
        r.unwrap_err().raw_os_error().unwrap()
    }

    #[test]
    fn format_makes_empty_root() {
        let (_t, fs) = fresh(8);
        let root = fs.read_inode(ROOT_INO).unwrap();
        assert!(root.is_dir());
        assert!(fs.readdir(ROOT_INO).unwrap().is_empty());
    }

    #[test]
    fn small_file_roundtrip() {
        let (_t, fs) = fresh(8);
        let f = fs.create(ROOT_INO, b"hello.txt", Kind::File, 0o644, 1000, 1000).unwrap();
        assert_eq!(fs.write(f, 0, b"quack quack").unwrap(), 11);
        assert_eq!(fs.read(f, 0, 100).unwrap(), b"quack quack");
        assert_eq!(fs.read(f, 6, 5).unwrap(), b"quack");
        assert_eq!(fs.lookup(ROOT_INO, b"hello.txt").unwrap(), Some(f));
        assert_eq!(fs.lookup(ROOT_INO, b"nope").unwrap(), None);
        assert_eq!(fs.read_inode(f).unwrap().uid, 1000);
    }

    #[test]
    fn big_file_uses_indirect_and_double_indirect() {
        let (_t, fs) = fresh(32);
        let f = fs.create(ROOT_INO, b"big", Kind::File, 0o644, 0, 0).unwrap();
        let data: Vec<u8> = (0..6 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        assert_eq!(fs.write(f, 0, &data).unwrap(), data.len());
        assert!(fs.read_inode(f).unwrap().dindirect != 0);
        assert_eq!(fs.read(f, 0, data.len()).unwrap(), data);
        assert_eq!(fs.read(f, 5_000_000, 1000).unwrap(), data[5_000_000..5_001_000]);
    }

    #[test]
    fn unlink_gives_space_back() {
        let (_t, fs) = fresh(32);
        // warm up: the root dir grows by one block for its first entry and keeps it
        fs.create(ROOT_INO, b"warm", Kind::File, 0o644, 0, 0).unwrap();
        fs.unlink(ROOT_INO, b"warm").unwrap();
        let before = fs.statfs().unwrap();
        let f = fs.create(ROOT_INO, b"f", Kind::File, 0o644, 0, 0).unwrap();
        fs.write(f, 0, &vec![7u8; 6 * 1024 * 1024]).unwrap();
        assert!(fs.statfs().unwrap().bfree < before.bfree);
        fs.unlink(ROOT_INO, b"f").unwrap();
        let after = fs.statfs().unwrap();
        assert_eq!(after.bfree, before.bfree);
        assert_eq!(after.ffree, before.ffree);
        assert_eq!(fs.lookup(ROOT_INO, b"f").unwrap(), None);
    }

    #[test]
    fn dirs_rename_rmdir() {
        let (_t, fs) = fresh(8);
        let a = fs.create(ROOT_INO, b"a", Kind::Dir, 0o755, 0, 0).unwrap();
        let b = fs.create(a, b"b", Kind::Dir, 0o755, 0, 0).unwrap();
        let f = fs.create(b, b"f", Kind::File, 0o644, 0, 0).unwrap();
        fs.write(f, 0, b"data").unwrap();
        assert_eq!(fs.read_inode(a).unwrap().nlink, 3);
        assert_eq!(errno(fs.rmdir(a, b"b")), libc::ENOTEMPTY);
        assert_eq!(errno(fs.rename(ROOT_INO, b"a", b, b"a2")), libc::EINVAL);
        fs.rename(b, b"f", a, b"f").unwrap();
        assert_eq!(fs.read(fs.lookup(a, b"f").unwrap().unwrap(), 0, 10).unwrap(), b"data");
        fs.rmdir(a, b"b").unwrap();
        assert_eq!(errno(fs.rmdir(ROOT_INO, b"a")), libc::ENOTEMPTY);
        fs.rename(a, b"f", ROOT_INO, b"moved").unwrap();
        fs.rmdir(ROOT_INO, b"a").unwrap();
        assert_eq!(fs.readdir(ROOT_INO).unwrap().len(), 1);
        assert_eq!(fs.read_inode(ROOT_INO).unwrap().nlink, 2);
    }

    #[test]
    fn rename_over_existing_file() {
        let (_t, fs) = fresh(8);
        let x = fs.create(ROOT_INO, b"x", Kind::File, 0o644, 0, 0).unwrap();
        let y = fs.create(ROOT_INO, b"y", Kind::File, 0o644, 0, 0).unwrap();
        fs.write(x, 0, b"X").unwrap();
        fs.write(y, 0, b"Y").unwrap();
        fs.rename(ROOT_INO, b"x", ROOT_INO, b"y").unwrap();
        let got = fs.lookup(ROOT_INO, b"y").unwrap().unwrap();
        assert_eq!(fs.read(got, 0, 10).unwrap(), b"X");
        assert_eq!(fs.lookup(ROOT_INO, b"x").unwrap(), None);
    }

    #[test]
    fn truncate_shrink_then_grow_is_zeroed() {
        let (_t, fs) = fresh(8);
        let f = fs.create(ROOT_INO, b"t", Kind::File, 0o644, 0, 0).unwrap();
        fs.write(f, 0, &vec![9u8; 10000]).unwrap();
        fs.set_size(f, 5000).unwrap();
        assert_eq!(fs.read(f, 0, 99999).unwrap().len(), 5000);
        fs.set_size(f, 12000).unwrap();
        let d = fs.read(f, 0, 99999).unwrap();
        assert_eq!(d.len(), 12000);
        assert!(d[..5000].iter().all(|&x| x == 9));
        assert!(d[5000..].iter().all(|&x| x == 0));
    }

    #[test]
    fn sparse_file_reads_zeros() {
        let (_t, fs) = fresh(16);
        let f = fs.create(ROOT_INO, b"s", Kind::File, 0o644, 0, 0).unwrap();
        fs.write(f, 100_000, b"end").unwrap();
        let d = fs.read(f, 0, 100_003).unwrap();
        assert!(d[..100_000].iter().all(|&x| x == 0));
        assert_eq!(&d[100_000..], b"end");
    }

    #[test]
    fn survives_reopen() {
        let (t, fs) = fresh(8);
        let f = fs.create(ROOT_INO, b"keep", Kind::File, 0o600, 5, 6).unwrap();
        fs.write(f, 0, b"still here").unwrap();
        drop(fs);
        let fs2 = DuckFs::open(&t.0).unwrap();
        let f2 = fs2.lookup(ROOT_INO, b"keep").unwrap().unwrap();
        assert_eq!(fs2.read(f2, 0, 100).unwrap(), b"still here");
        assert_eq!(fs2.read_inode(f2).unwrap().gid, 6);
    }

    #[test]
    fn enospc_is_clean() {
        let (_t, fs) = fresh(1); // 256 blocks
        let f = fs.create(ROOT_INO, b"fill", Kind::File, 0o644, 0, 0).unwrap();
        let n = fs.write(f, 0, &vec![1u8; 2 * 1024 * 1024]).unwrap();
        assert!(n > 0 && n < 2 * 1024 * 1024);
        assert_eq!(errno(fs.write(f, n as u64, &vec![1u8; 8192])), libc::ENOSPC);
        fs.unlink(ROOT_INO, b"fill").unwrap();
        let g = fs.create(ROOT_INO, b"again", Kind::File, 0o644, 0, 0).unwrap();
        assert_eq!(fs.write(g, 0, b"works").unwrap(), 5);
    }

    #[test]
    fn name_rules() {
        let (_t, fs) = fresh(8);
        let long = vec![b'a'; MAX_NAME + 1];
        assert_eq!(errno(fs.create(ROOT_INO, &long, Kind::File, 0o644, 0, 0)), libc::ENAMETOOLONG);
        fs.create(ROOT_INO, b"dup", Kind::File, 0o644, 0, 0).unwrap();
        assert_eq!(errno(fs.create(ROOT_INO, b"dup", Kind::File, 0o644, 0, 0)), libc::EEXIST);
        assert_eq!(errno(fs.create(ROOT_INO, b"..", Kind::Dir, 0o755, 0, 0)), libc::EEXIST);
        fs.create(ROOT_INO, &[0xff, 0xfe, b'x'], Kind::File, 0o644, 0, 0).unwrap();
        let max = vec![b'm'; MAX_NAME];
        fs.create(ROOT_INO, &max, Kind::File, 0o644, 0, 0).unwrap();
        assert_eq!(fs.lookup(ROOT_INO, &max).unwrap().is_some(), true);
    }

    #[test]
    fn many_files_in_one_dir() {
        let (_t, fs) = fresh(16);
        for i in 0..300 {
            fs.create(ROOT_INO, format!("file{i}").as_bytes(), Kind::File, 0o644, 0, 0).unwrap();
        }
        assert_eq!(fs.readdir(ROOT_INO).unwrap().len(), 300);
        for i in (0..300).step_by(2) {
            fs.unlink(ROOT_INO, format!("file{i}").as_bytes()).unwrap();
        }
        assert_eq!(fs.readdir(ROOT_INO).unwrap().len(), 150);
        fs.create(ROOT_INO, b"reuse", Kind::File, 0o644, 0, 0).unwrap();
        assert_eq!(fs.readdir(ROOT_INO).unwrap().len(), 151);
    }
}
