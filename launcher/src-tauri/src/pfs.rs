//! Read-only reader for PS4 fake PKG files (zero passcode): PKG header and entry table, outer PFS (AES-XTS) -> `pfs_image.dat`
//! (PFSC, zlib blocks) -> inner PFS.

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};

pub const PKG_MAGIC: u32 = 0x7F43_4E54;
const SECTOR: u64 = 0x1000;
const FIRST_ENCRYPTED_SECTOR: u64 = 16;
const CHUNK: u64 = if cfg!(test) { 64 << 10 } else { 8 << 20 };

pub const ENTRY_NAMES: &[(u32, &str)] = &[
    (0x0001, "digests"), (0x0010, "entry_keys"), (0x0020, "image_key"), (0x0080, "general_digests"),
    (0x0100, "metas"), (0x0200, "entry_names"), (0x0400, "license.dat"), (0x0401, "license.info"),
    (0x0402, "nptitle.dat"), (0x0403, "npbind.dat"), (0x0404, "selfinfo.dat"), (0x0406, "imageinfo.dat"),
    (0x0407, "target-deltainfo.dat"), (0x0408, "origin-deltainfo.dat"), (0x0409, "psreserved.dat"),
    (0x1000, "param.sfo"), (0x1001, "playgo-chunk.dat"), (0x1002, "playgo-chunk.sha"),
    (0x1003, "playgo-manifest.xml"), (0x1004, "pronunciation.xml"), (0x1005, "pronunciation.sig"),
    (0x1006, "pic1.png"), (0x1007, "pubtoolinfo.dat"), (0x1008, "app/playgo-chunk.dat"),
    (0x100B, "app/playgo-manifest.xml"), (0x100C, "shareparam.json"), (0x100D, "shareoverlayimage.png"),
    (0x100E, "save_data.png"), (0x100F, "shareprivacyguardimage.png"), (0x1200, "icon0.png"),
    (0x1220, "pic0.png"), (0x1240, "snd0.at9"), (0x1260, "changeinfo/changeinfo.xml"),
    (0x1280, "icon0.dds"), (0x12A0, "pic0.dds"), (0x12C0, "pic1.dds"),
];

pub fn entry_name(id: u32) -> Option<&'static str> {
    ENTRY_NAMES.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}
fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Reads at `off` until `buf` is full or the file ends; the number of bytes read.
pub fn read_at(f: &File, off: u64, buf: &mut [u8]) -> io::Result<usize> {
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;
    #[cfg(unix)]
    use std::os::unix::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        #[cfg(windows)]
        let n = f.seek_read(&mut buf[done..], off + done as u64)?;
        #[cfg(unix)]
        let n = f.read_at(&mut buf[done..], off + done as u64)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Ok(done)
}

fn read_vec(f: &File, off: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0u8; len];
    let n = read_at(f, off, &mut v)?;
    v.truncate(n);
    Ok(v)
}

// ---------- PKG header and entries ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub content_id: [u8; 36],
    pub entry_count: u32,
    pub table_offset: u32,
    pub pfs_offset: u64,
    pub pfs_size: u64,
}

impl Header {
    pub fn content_id(&self) -> String {
        String::from_utf8_lossy(&self.content_id).trim_end_matches('\0').to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: u32,
    pub flags1: u32,
    pub offset: u32,
    pub size: u32,
}

impl Entry {
    pub fn encrypted(&self) -> bool {
        self.flags1 & 0x8000_0000 != 0
    }
}

pub fn parse_header(h: &[u8]) -> io::Result<Header> {
    if h.len() < 0x420 {
        return Err(bad("the file is too short for a PKG header"));
    }
    if be32(h, 0) != PKG_MAGIC {
        return Err(bad("not a PS4 PKG file"));
    }
    Ok(Header {
        content_id: h[0x40..0x64].try_into().unwrap(),
        entry_count: be32(h, 0x10),
        table_offset: be32(h, 0x18),
        pfs_offset: be64(h, 0x410),
        pfs_size: be64(h, 0x418),
    })
}

pub fn parse_entries(table: &[u8], count: u32) -> io::Result<Vec<Entry>> {
    let count = count as usize;
    if table.len() < count * 32 {
        return Err(bad("truncated PKG entry table"));
    }
    Ok((0..count)
        .map(|i| {
            let e = &table[i * 32..i * 32 + 32];
            Entry { id: be32(e, 0), flags1: be32(e, 8), offset: be32(e, 16), size: be32(e, 20) }
        })
        .collect())
}

pub fn read_header(f: &File) -> io::Result<Header> {
    parse_header(&read_vec(f, 0, 0x1000)?)
}

pub fn read_entries(f: &File, h: &Header) -> io::Result<Vec<Entry>> {
    if h.entry_count > 4096 {
        return Err(bad("implausible PKG entry count"));
    }
    parse_entries(&read_vec(f, h.table_offset as u64, h.entry_count as usize * 32)?, h.entry_count)
}

/// The unencrypted param.sfo entry of a PKG.
pub fn read_param_sfo(path: &Path) -> io::Result<Vec<u8>> {
    let f = File::open(path)?;
    let h = read_header(&f)?;
    let e = read_entries(&f, &h)?
        .into_iter()
        .find(|e| e.id == 0x1000 && !e.encrypted())
        .ok_or_else(|| bad("the PKG has no readable param.sfo"))?;
    let data = read_vec(&f, e.offset as u64, e.size as usize)?;
    if data.len() != e.size as usize {
        return Err(bad("truncated param.sfo"));
    }
    Ok(data)
}

/// Unencrypted entries with a known name and id >= 0x1000: they go in `sce_sys/`.
pub fn sce_sys_entries(entries: &[Entry]) -> Vec<(&'static str, u64, u64)> {
    entries
        .iter()
        .filter(|e| e.id >= 0x1000 && !e.encrypted())
        .filter_map(|e| entry_name(e.id).map(|n| (n, e.offset as u64, e.size as u64)))
        .collect()
}

// ---------- Keys and AES-XTS ----------

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// HMAC-SHA256 for keys of at most one block (64 bytes), the only ones the launcher uses.
pub(crate) fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    assert!(key.len() <= 64);
    let mut k = [0u8; 64];
    k[..key.len()].copy_from_slice(key);
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let inner = sha256(&[&ipad, msg]);
    sha256(&[&opad, &inner])
}

/// `compute_keys(cid, passcode, index)` of the Python tools.
fn compute_keys(content_id: &[u8; 36], passcode: &[u8], index: u32) -> [u8; 32] {
    let mut c = [0u8; 48];
    c[..36].copy_from_slice(content_id);
    sha256(&[&sha256(&[&index.to_be_bytes()]), &sha256(&[&c]), passcode])
}

/// The outer PFS keys from the content id and the PFS seed.
pub fn outer_xts(content_id: &[u8; 36], seed: &[u8]) -> Xts {
    let (data, tweak) = outer_keys(content_id, seed);
    Xts::new(&data, &tweak)
}

/// (data key, tweak key).
fn outer_keys(content_id: &[u8; 36], seed: &[u8]) -> ([u8; 16], [u8; 16]) {
    let ekpfs = compute_keys(content_id, &[b'0'; 32], 1);
    let key = hmac_sha256(&ekpfs, seed);
    let mut msg = 1u32.to_le_bytes().to_vec();
    msg.extend_from_slice(seed);
    let r = hmac_sha256(&key, &msg);
    (r[16..32].try_into().unwrap(), r[..16].try_into().unwrap())
}

/// AES-128-XTS decryption of 4 KiB sectors, tweak = sector number (little endian, 128 bits).
pub struct Xts {
    data: Aes128,
    tweak: Aes128,
}

impl Xts {
    pub fn new(data_key: &[u8], tweak_key: &[u8]) -> Xts {
        Xts { data: Aes128::new(GenericArray::from_slice(data_key)), tweak: Aes128::new(GenericArray::from_slice(tweak_key)) }
    }

    /// Decrypts one data unit in place; `buf.len()` must be a multiple of 16.
    pub fn decrypt_unit(&self, unit: u64, buf: &mut [u8]) {
        let mut t = [0u8; 16];
        t[..8].copy_from_slice(&unit.to_le_bytes());
        let mut tb = GenericArray::clone_from_slice(&t);
        self.tweak.encrypt_block(&mut tb);
        let mut tw = u128::from_le_bytes(tb.into());
        let n = buf.len() / 16;
        let mut tweaks = Vec::with_capacity(n);
        let mut blocks: Vec<GenericArray<u8, aes::cipher::consts::U16>> = Vec::with_capacity(n);
        for i in 0..n {
            let x = u128::from_le_bytes(buf[i * 16..i * 16 + 16].try_into().unwrap()) ^ tw;
            blocks.push(GenericArray::clone_from_slice(&x.to_le_bytes()));
            tweaks.push(tw);
            let carry = tw >> 127;
            tw <<= 1;
            if carry != 0 {
                tw ^= 0x87;
            }
        }
        self.data.decrypt_blocks(&mut blocks);
        for i in 0..n {
            let x = u128::from_le_bytes(blocks[i].into()) ^ tweaks[i];
            buf[i * 16..i * 16 + 16].copy_from_slice(&x.to_le_bytes());
        }
    }

    /// Decrypts whole sectors starting at `first`, leaving the first 16 sectors and a short tail as they are (XtsSource).
    pub fn decrypt_sectors(&self, first: u64, buf: &mut [u8]) {
        for (i, s) in buf.chunks_mut(SECTOR as usize).enumerate() {
            let n = first + i as u64;
            if n >= FIRST_ENCRYPTED_SECTOR && s.len() == SECTOR as usize {
                self.decrypt_unit(n, s);
            }
        }
    }
}

// ---------- Random-access sources ----------

pub trait Source: Send + Sync {
    /// Reads at `off`; fewer bytes than asked only at the end of the data.
    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize>;

    fn read_vec(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0u8; len];
        let n = self.read(off, &mut v)?;
        v.truncate(n);
        Ok(v)
    }
}

/// The outer PFS: the PKG bytes from `pfs_offset`, decrypted sector by sector.
pub struct Outer {
    file: Arc<File>,
    base: u64,
    xts: Arc<Xts>,
    cache: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
}

impl Outer {
    fn sector(&self, s: u64) -> io::Result<Arc<Vec<u8>>> {
        if let Some(v) = self.cache.lock().unwrap().get(&s) {
            return Ok(v.clone());
        }
        let mut raw = read_vec(&self.file, self.base + s * SECTOR, SECTOR as usize)?;
        self.xts.decrypt_sectors(s, &mut raw);
        let v = Arc::new(raw);
        let mut c = self.cache.lock().unwrap();
        if c.len() >= 4096 {
            c.clear();
        }
        c.insert(s, v.clone());
        Ok(v)
    }
}

impl Source for Outer {
    fn read(&self, mut off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            let sec = self.sector(off / SECTOR)?;
            let o = (off % SECTOR) as usize;
            if o >= sec.len() {
                break;
            }
            let n = (sec.len() - o).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&sec[o..o + n]);
            done += n;
            off += n as u64;
        }
        Ok(done)
    }
}

/// A window of another source, clamped to `size` (SubSource).
pub struct Sub<'a, S: Source + ?Sized> {
    src: &'a S,
    base: u64,
    size: u64,
}

impl<S: Source + ?Sized> Source for Sub<'_, S> {
    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        if off >= self.size {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(self.size - off) as usize;
        self.src.read(self.base + off, &mut buf[..n])
    }
}

/// Decodes one PFSC block: stored when it has the block size, zeros when empty, else zlib (raw deflate after 2 bytes as a fallback).
pub fn decode_block(raw: &[u8], bsz: usize) -> io::Result<Vec<u8>> {
    if raw.len() == bsz {
        return Ok(raw.to_vec());
    }
    if raw.is_empty() {
        return Ok(vec![0; bsz]);
    }
    inflate(raw, true, bsz).or_else(|_| inflate(raw.get(2..).unwrap_or(&[]), false, bsz))
}

fn inflate(raw: &[u8], zlib: bool, hint: usize) -> io::Result<Vec<u8>> {
    use flate2::{Decompress, FlushDecompress, Status};
    let mut d = Decompress::new(zlib);
    let mut out = Vec::with_capacity(hint);
    loop {
        let before = d.total_in();
        let status = d
            .decompress_vec(&raw[d.total_in() as usize..], &mut out, FlushDecompress::Finish)
            .map_err(|e| bad(format!("corrupt compressed block: {e}")))?;
        match status {
            Status::StreamEnd => return Ok(out),
            _ if out.len() == out.capacity() => out.reserve(hint.max(4096)),
            _ if d.total_in() == before || d.total_in() as usize >= raw.len() => {
                return Err(bad("truncated compressed block"))
            }
            _ => {}
        }
    }
}

/// `pfs_image.dat`: PFSC-compressed or plain, inside the outer PFS.
pub struct Image {
    outer: Arc<Outer>,
    /// Offset of `pfs_image.dat` in the outer PFS.
    base: u64,
    /// Size of `pfs_image.dat` in the outer PFS.
    stored: u64,
    pfsc: Option<Pfsc>,
    cache: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
}

struct Pfsc {
    bsz: u64,
    size: u64,
    offs: Vec<u64>,
}

impl Image {
    pub fn size(&self) -> u64 {
        self.pfsc.as_ref().map(|p| p.size).unwrap_or(self.stored)
    }

    fn stored_read(&self, off: u64, len: u64) -> io::Result<Vec<u8>> {
        let sub = Sub { src: &*self.outer, base: self.base, size: self.stored };
        sub.read_vec(off, len as usize)
    }

    fn block(&self, p: &Pfsc, i: u64) -> io::Result<Arc<Vec<u8>>> {
        if let Some(v) = self.cache.lock().unwrap().get(&i) {
            return Ok(v.clone());
        }
        let (a, b) = (p.offs[i as usize], p.offs[i as usize + 1]);
        if b < a {
            return Err(bad("corrupt PFSC block table"));
        }
        let raw = self.stored_read(a, b - a)?;
        let v = Arc::new(decode_block(&raw, p.bsz as usize)?);
        let mut c = self.cache.lock().unwrap();
        if c.len() >= 256 {
            c.clear();
        }
        c.insert(i, v.clone());
        Ok(v)
    }
}

impl Source for Image {
    fn read(&self, mut off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let Some(p) = &self.pfsc else {
            return Sub { src: &*self.outer, base: self.base, size: self.stored }.read(off, buf);
        };
        if off >= p.size {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(p.size - off) as usize;
        let mut done = 0;
        while done < want {
            let blk = self.block(p, off / p.bsz)?;
            let o = (off % p.bsz) as usize;
            if o >= blk.len() {
                break;
            }
            let n = (blk.len() - o).min(want - done);
            buf[done..done + n].copy_from_slice(&blk[o..o + n]);
            done += n;
            off += n as u64;
        }
        Ok(done)
    }
}

fn parse_pfsc(img: &dyn Source) -> io::Result<Option<Pfsc>> {
    let h = img.read_vec(0, 0x30)?;
    if h.len() < 4 || &h[..4] != b"PFSC" {
        return Ok(None);
    }
    if h.len() < 0x30 {
        return Err(bad("truncated PFSC header"));
    }
    let bsz = le32(&h, 0x0C) as u64;
    let table = le64(&h, 0x18);
    let size = le64(&h, 0x28);
    if bsz == 0 || bsz > 16 << 20 {
        return Err(bad("implausible PFSC block size"));
    }
    let nblocks = size.div_ceil(bsz);
    let raw = img.read_vec(table, ((nblocks + 1) * 8) as usize)?;
    if raw.len() as u64 != (nblocks + 1) * 8 {
        return Err(bad("truncated PFSC block table"));
    }
    let offs = raw.chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
    Ok(Some(Pfsc { bsz, size, offs }))
}

// ---------- PFS ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Inode {
    pub mode: u16,
    pub flags: u32,
    pub size: u64,
    pub csize: u64,
    pub db: [u64; 12],
}

pub struct Pfs<S: Source> {
    pub src: S,
    pub bsz: u64,
    isz: u64,
    ptr: (u64, u64),
    per_block: u64,
}

impl<S: Source> Pfs<S> {
    pub fn new(src: S) -> io::Result<Pfs<S>> {
        let h = src.read_vec(0, 0x400)?;
        if h.len() < 0x400 {
            return Err(bad("truncated PFS header"));
        }
        let mode = le16(&h, 0x1C);
        let bsz = le32(&h, 0x20) as u64;
        if !(0x1000..=0x100000).contains(&bsz) || !bsz.is_power_of_two() {
            return Err(bad("implausible PFS block size (wrong key?)"));
        }
        let (signed, is64) = (mode & 1 != 0, mode & 2 != 0);
        let (isz, ptr) = match (signed, is64) {
            (true, true) => (0x64 + 17 * 40, (32, 40)),
            (true, false) => (0x64 + 17 * 36, (32, 36)),
            (false, true) => (0x64 + 17 * 8, (0, 8)),
            (false, false) => (0x64 + 17 * 4, (0, 4)),
        };
        Ok(Pfs { src, bsz, isz, ptr, per_block: bsz / isz })
    }

    pub fn inode(&self, n: u64) -> io::Result<Inode> {
        let (blk, idx) = (n / self.per_block, n % self.per_block);
        let d = self.src.read_vec((1 + blk) * self.bsz + idx * self.isz, self.isz as usize)?;
        if d.len() < self.isz as usize {
            return Err(bad("truncated PFS inode"));
        }
        let (skip, step) = self.ptr;
        let mut db = [0u64; 12];
        for (i, v) in db.iter_mut().enumerate() {
            let o = (0x64 + i as u64 * step + skip) as usize;
            *v = if step - skip == 8 { le64(&d, o) } else { le32(&d, o) as u64 };
        }
        Ok(Inode { mode: le16(&d, 0), flags: le32(&d, 4), size: le64(&d, 8), csize: le64(&d, 16), db })
    }

    /// Where a file's bytes are in the source: PKG-generated images are contiguous from db[0].
    pub fn data_range(&self, ino: &Inode) -> (u64, u64) {
        (ino.db[0] * self.bsz, ino.size)
    }

    pub fn dirents(&self, n: u64) -> io::Result<Vec<(String, u64, i32)>> {
        let ino = self.inode(n)?;
        if ino.size > 64 << 20 {
            return Err(bad("implausible PFS directory size"));
        }
        let (off, size) = self.data_range(&ino);
        let raw = Sub { src: &self.src, base: off, size }.read_vec(0, size as usize)?;
        let mut out = Vec::new();
        let mut o = 0usize;
        while o + 16 <= raw.len() {
            let inum = le32(&raw, o) as u64;
            let typ = le32(&raw, o + 4) as i32;
            let nlen = le32(&raw, o + 8) as i32;
            let esz = le32(&raw, o + 12) as usize;
            if esz == 0 {
                o = (o / self.bsz as usize + 1) * self.bsz as usize;
                continue;
            }
            let end = (o + 16 + nlen.max(0) as usize).min(raw.len());
            let name = String::from_utf8_lossy(&raw[o + 16..end]).into_owned();
            if typ != 4 && typ != 5 {
                out.push((name, inum, typ));
            }
            o += esz;
        }
        Ok(out)
    }

    pub fn find(&self, dir: u64, name: &str) -> io::Result<u64> {
        self.dirents(dir)?
            .into_iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, i, _)| i)
            .ok_or_else(|| bad(format!("{name} is missing in the PFS")))
    }

    /// Every file under `dir`, as "a/b/c" with its inode (directories are walked, not listed).
    pub fn walk(&self, dir: u64) -> io::Result<Vec<(String, Inode)>> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.walk_into(dir, "", &mut out, &mut seen, 0)?;
        Ok(out)
    }

    fn walk_into(
        &self,
        dir: u64,
        prefix: &str,
        out: &mut Vec<(String, Inode)>,
        seen: &mut std::collections::HashSet<u64>,
        depth: u32,
    ) -> io::Result<()> {
        if depth > 64 || !seen.insert(dir) {
            return Err(bad("PFS directory loop"));
        }
        for (name, inum, typ) in self.dirents(dir)? {
            let path = format!("{prefix}{name}");
            if typ == 3 {
                self.walk_into(inum, &format!("{path}/"), out, seen, depth + 1)?;
            } else {
                out.push((path, self.inode(inum)?));
            }
        }
        Ok(())
    }
}

impl<S: Source + ?Sized> Source for &S {
    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read(off, buf)
    }
}

impl<S: Source + ?Sized> Source for Arc<S> {
    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read(off, buf)
    }
}

// ---------- Package ----------

/// One file of the inner PFS: `offset` in `pfs_image.dat` (decompressed), `size` bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct PkgFile {
    pub path: String,
    pub offset: u64,
    pub size: u64,
}

pub struct Package {
    pub path: PathBuf,
    file: Arc<File>,
    pub entries: Vec<Entry>,
    outer: Arc<Outer>,
    inner: Pfs<Arc<Image>>,
    root: u64,
}

impl Package {
    pub fn open(path: &Path) -> io::Result<Package> {
        let file = Arc::new(File::open(path)?);
        let h = read_header(&file)?;
        let entries = read_entries(&file, &h)?;
        let seed = read_vec(&file, h.pfs_offset + 0x370, 16)?;
        if seed.len() != 16 {
            return Err(bad("truncated PFS"));
        }
        let outer = Arc::new(Outer {
            file: file.clone(),
            base: h.pfs_offset,
            xts: Arc::new(outer_xts(&h.content_id, &seed)),
            cache: Mutex::new(HashMap::new()),
        });
        let outer_pfs = Pfs::new(outer.clone())?;
        let uroot = outer_pfs.find(0, "uroot")?;
        let img_ino = outer_pfs.inode(outer_pfs.find(uroot, "pfs_image.dat")?)?;
        let (base, stored) = outer_pfs.data_range(&img_ino);
        let stored_src = Sub { src: &*outer, base, size: stored };
        let pfsc = parse_pfsc(&stored_src)?;
        let image = Arc::new(Image { outer: outer.clone(), base, stored, pfsc, cache: Mutex::new(HashMap::new()) });
        let inner = Pfs::new(image)?;
        let root = inner.find(0, "uroot")?;
        Ok(Package { path: path.to_path_buf(), file, entries, outer, inner, root })
    }

    pub fn files(&self) -> io::Result<Vec<PkgFile>> {
        Ok(self
            .inner
            .walk(self.root)?
            .into_iter()
            .map(|(path, ino)| {
                let (offset, size) = self.inner.data_range(&ino);
                PkgFile { path, offset, size }
            })
            .collect())
    }

    /// Bytes of an unencrypted PKG entry.
    pub fn entry_bytes(&self, offset: u64, size: u64) -> io::Result<Vec<u8>> {
        let v = read_vec(&self.file, offset, size as usize)?;
        if v.len() as u64 != size {
            return Err(bad("truncated PKG entry"));
        }
        Ok(v)
    }

    /// Streams `[start, end)` of `pfs_image.dat` to `sink` in order: one thread reads, `workers` decrypt and
    /// decompress. Returns false when `cancel` stopped it.
    pub fn stream(
        &self,
        start: u64,
        end: u64,
        workers: usize,
        cancel: &AtomicBool,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<bool> {
        let image = &*self.inner.src;
        let end = end.min(image.size());
        if start >= end {
            return Ok(true);
        }
        let jobs = self.jobs(image, start, end);
        let workers = workers.max(1);
        let slots = 2 * workers + 2;
        std::thread::scope(|scope| {
            let (token_tx, token_rx) = sync_channel::<()>(slots);
            for _ in 0..slots {
                let _ = token_tx.send(());
            }
            let (work_tx, work_rx) = sync_channel::<(usize, io::Result<Vec<u8>>)>(slots);
            let (done_tx, done_rx) = std::sync::mpsc::channel::<(usize, io::Result<Vec<u8>>)>();
            let jobs = &jobs;
            let file = &self.file;
            let base = self.outer.base;
            scope.spawn(move || {
                for (i, job) in jobs.iter().enumerate() {
                    if token_rx.recv().is_err() || cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    let len = ((job.last_sector - job.first_sector) * SECTOR) as usize;
                    let raw = read_vec(file, base + job.first_sector * SECTOR, len);
                    let failed = raw.is_err();
                    if work_tx.send((i, raw)).is_err() || failed {
                        return;
                    }
                }
            });
            let work_rx = Arc::new(Mutex::new(work_rx));
            for _ in 0..workers {
                let rx: Arc<Mutex<Receiver<(usize, io::Result<Vec<u8>>)>>> = work_rx.clone();
                let tx = done_tx.clone();
                scope.spawn(move || loop {
                    let next = rx.lock().unwrap().recv();
                    let Ok((i, raw)) = next else { return };
                    let out = raw.and_then(|raw| self.run_job(image, &jobs[i], raw));
                    if tx.send((i, out)).is_err() {
                        return;
                    }
                });
            }
            drop(done_tx);
            let mut pending = std::collections::BTreeMap::new();
            let mut next = 0usize;
            while next < jobs.len() {
                if cancel.load(Ordering::SeqCst) {
                    return Ok(false);
                }
                while let Some(out) = pending.remove(&next) {
                    let out: Vec<u8> = out;
                    sink(&out)?;
                    next += 1;
                    let _ = token_tx.send(());
                    if cancel.load(Ordering::SeqCst) {
                        return Ok(false);
                    }
                }
                if next >= jobs.len() {
                    break;
                }
                match done_rx.recv() {
                    Ok((i, out)) => {
                        pending.insert(i, out?);
                    }
                    Err(_) => {
                        if cancel.load(Ordering::SeqCst) {
                            return Ok(false);
                        }
                        return Err(bad("the PKG reader stopped early"));
                    }
                }
            }
            Ok(true)
        })
    }

    fn jobs(&self, image: &Image, start: u64, end: u64) -> Vec<Job> {
        let mut jobs = Vec::new();
        let sectors = |a: u64, b: u64| ((image.base + a) / SECTOR, (image.base + b).div_ceil(SECTOR));
        match &image.pfsc {
            Some(p) => {
                let per = (CHUNK / p.bsz).max(1);
                let (first, last) = (start / p.bsz, (end - 1) / p.bsz + 1);
                let mut b0 = first;
                while b0 < last {
                    let b1 = (b0 + per).min(last);
                    let a = p.offs[b0 as usize].min(image.stored);
                    let b = p.offs[b1 as usize].min(image.stored).max(a);
                    let (s0, s1) = sectors(a, b);
                    let lo = start.max(b0 * p.bsz);
                    let hi = end.min(b1 * p.bsz);
                    jobs.push(Job { first_sector: s0, last_sector: s1.max(s0), kind: JobKind::Blocks { b0, b1 }, lo, hi });
                    b0 = b1;
                }
            }
            None => {
                let end = end.min(image.stored);
                let mut a = start;
                while a < end {
                    let b = (a + CHUNK).min(end);
                    let (s0, s1) = sectors(a, b);
                    jobs.push(Job { first_sector: s0, last_sector: s1, kind: JobKind::Bytes, lo: a, hi: b });
                    a = b;
                }
            }
        }
        jobs
    }

    fn run_job(&self, image: &Image, job: &Job, mut raw: Vec<u8>) -> io::Result<Vec<u8>> {
        self.outer.xts.decrypt_sectors(job.first_sector, &mut raw);
        let at = |pos: u64| -> io::Result<usize> {
            let o = image.base + pos;
            let rel = o.checked_sub(job.first_sector * SECTOR).ok_or_else(|| bad("bad chunk offset"))? as usize;
            Ok(rel)
        };
        match job.kind {
            JobKind::Bytes => {
                let (a, b) = (at(job.lo)?, at(job.hi)?);
                raw.get(a..b).map(|s| s.to_vec()).ok_or_else(|| bad("the PKG file ends early"))
            }
            JobKind::Blocks { b0, b1 } => {
                let p = image.pfsc.as_ref().unwrap();
                let mut out = Vec::with_capacity(((b1 - b0) * p.bsz) as usize);
                for i in b0..b1 {
                    let a = p.offs[i as usize].min(image.stored);
                    let b = p.offs[i as usize + 1].min(image.stored);
                    if b < a {
                        return Err(bad("corrupt PFSC block table"));
                    }
                    let slice = raw.get(at(a)?..at(b)?).ok_or_else(|| bad("the PKG file ends early"))?;
                    let block = decode_block(slice, p.bsz as usize)?;
                    if block.len() as u64 != p.bsz && i + 1 != p.offs.len() as u64 - 1 {
                        return Err(bad("corrupt compressed block size"));
                    }
                    out.extend_from_slice(&block);
                }
                let lo = (job.lo - b0 * p.bsz) as usize;
                let hi = ((job.hi - b0 * p.bsz) as usize).min(out.len());
                if lo > hi {
                    return Err(bad("the PKG file ends early"));
                }
                out.truncate(hi);
                out.drain(..lo);
                Ok(out)
            }
        }
    }
}

struct Job {
    first_sector: u64,
    last_sector: u64,
    kind: JobKind,
    lo: u64,
    hi: u64,
}

enum JobKind {
    Blocks { b0: u64, b1: u64 },
    Bytes,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn xts_matches_ieee_1619_vectors() {
        let x = Xts::new(&[0; 16], &[0; 16]);
        let mut buf = hex("917cf69ebd68b2ec9b9fe9a3eadda692cd43d2f59598ed858c02c2652fbf922e");
        x.decrypt_unit(0, &mut buf);
        assert_eq!(buf, vec![0u8; 32]);
        let x = Xts::new(&[0x11; 16], &[0x22; 16]);
        let mut buf = hex("c454185e6a16936e39334038acef838bfb186fff7480adc4289382ecd6d394f0");
        x.decrypt_unit(0x3333333333, &mut buf);
        assert_eq!(buf, vec![0x44u8; 32]);
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        let mut key = [0u8; 32];
        key[..20].copy_from_slice(&[0x0b; 20]);
        assert_eq!(
            hmac_sha256(&key, b"Hi There").to_vec(),
            hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
        );
    }

    #[test]
    fn key_derivation_matches_pfs_py() {
        let (data, tweak) = outer_keys(b"EP0000-CUSA11182_00-TESTTESTTESTTEST", &(0..16).collect::<Vec<u8>>());
        let digest = sha256(&[&data, &tweak]);
        assert_eq!(digest.to_vec(), hex("72762108a94c77e9f0684ad2ca1006fb5322bc1dec1b4ff16ad3cbffa3cd2191"));
    }

    pub fn encrypt_unit(x_data: &[u8; 16], x_tweak: &[u8; 16], unit: u64, buf: &mut [u8]) {
        let data = Aes128::new(GenericArray::from_slice(x_data));
        let tweak = Aes128::new(GenericArray::from_slice(x_tweak));
        let mut t = [0u8; 16];
        t[..8].copy_from_slice(&unit.to_le_bytes());
        let mut tb = GenericArray::clone_from_slice(&t);
        tweak.encrypt_block(&mut tb);
        let mut tw = u128::from_le_bytes(tb.into());
        for c in buf.chunks_mut(16) {
            let x = u128::from_le_bytes(c.try_into().unwrap()) ^ tw;
            let mut b = GenericArray::clone_from_slice(&x.to_le_bytes());
            data.encrypt_block(&mut b);
            let y = u128::from_le_bytes(b.into()) ^ tw;
            c.copy_from_slice(&y.to_le_bytes());
            let carry = tw >> 127;
            tw <<= 1;
            if carry != 0 {
                tw ^= 0x87;
            }
        }
    }

    #[test]
    fn xts_round_trip_on_a_sector() {
        let (k1, k2) = ([7u8; 16], [9u8; 16]);
        let plain: Vec<u8> = (0..4096u32).map(|i| (i * 31 % 251) as u8).collect();
        let mut buf = plain.clone();
        encrypt_unit(&k1, &k2, 42, &mut buf);
        assert_ne!(buf, plain);
        Xts::new(&k1, &k2).decrypt_sectors(42, &mut buf);
        assert_eq!(buf, plain);
        let mut early = plain.clone();
        Xts::new(&k1, &k2).decrypt_sectors(3, &mut early);
        assert_eq!(early, plain, "the first 16 sectors are not encrypted");
    }

    #[test]
    fn header_and_entries() {
        let mut h = vec![0u8; 0x1000];
        h[0..4].copy_from_slice(&PKG_MAGIC.to_be_bytes());
        h[0x10..0x14].copy_from_slice(&2u32.to_be_bytes());
        h[0x18..0x1C].copy_from_slice(&0x800u32.to_be_bytes());
        h[0x40..0x64].copy_from_slice(b"EP0000-CUSA11182_00-TESTTESTTESTTEST");
        h[0x410..0x418].copy_from_slice(&0x10000u64.to_be_bytes());
        h[0x418..0x420].copy_from_slice(&0x20000u64.to_be_bytes());
        let hd = parse_header(&h).unwrap();
        assert_eq!(hd.content_id(), "EP0000-CUSA11182_00-TESTTESTTESTTEST");
        assert_eq!((hd.entry_count, hd.table_offset, hd.pfs_offset, hd.pfs_size), (2, 0x800, 0x10000, 0x20000));
        let mut t = vec![0u8; 64];
        t[0..4].copy_from_slice(&0x1000u32.to_be_bytes());
        t[16..20].copy_from_slice(&0x900u32.to_be_bytes());
        t[20..24].copy_from_slice(&0x40u32.to_be_bytes());
        t[32..36].copy_from_slice(&0x0010u32.to_be_bytes());
        t[40..44].copy_from_slice(&0x8000_0000u32.to_be_bytes());
        let e = parse_entries(&t, 2).unwrap();
        assert_eq!(e[0], Entry { id: 0x1000, flags1: 0, offset: 0x900, size: 0x40 });
        assert!(e[1].encrypted());
        assert_eq!(sce_sys_entries(&e), vec![("param.sfo", 0x900, 0x40)]);
        h[0] = 0;
        assert!(parse_header(&h).is_err());
    }

    struct Mem(Vec<u8>);
    impl Source for Mem {
        fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
            let o = (off as usize).min(self.0.len());
            let n = buf.len().min(self.0.len() - o);
            buf[..n].copy_from_slice(&self.0[o..o + n]);
            Ok(n)
        }
    }

    /// A tiny unsigned 32-bit PFS: inode table in block 1, root dir (inode 0) holds "uroot" (inode 1),
    /// which holds "a.txt" (inode 2) and directory "d" (inode 3) with "b.bin" (inode 4).
    pub fn tiny_pfs(bsz: usize, first_block: u32, files: &[(&str, &[u8])]) -> Vec<u8> {
        let isz = 0x64 + 17 * 4;
        let mut img = vec![0u8; bsz * 16];
        img[0x1C..0x1E].copy_from_slice(&0u16.to_le_bytes());
        img[0x20..0x24].copy_from_slice(&(bsz as u32).to_le_bytes());
        let put_inode =|img: &mut Vec<u8>, n: usize, size: u64, block: u32| {
            let o = bsz + n * isz;
            img[o + 8..o + 16].copy_from_slice(&size.to_le_bytes());
            img[o + 16..o + 24].copy_from_slice(&size.to_le_bytes());
            img[o + 0x64..o + 0x68].copy_from_slice(&block.to_le_bytes());
        };
        let dir = |entries: &[(&str, u32, i32)]| {
            let mut d = Vec::new();
            for (name, inum, typ) in [(".", 0, 4), ("..", 0, 5)].iter().chain(entries.iter()) {
                let esz = (16 + name.len()).div_ceil(8) * 8;
                let mut e = vec![0u8; esz];
                e[0..4].copy_from_slice(&inum.to_le_bytes());
                e[4..8].copy_from_slice(&typ.to_le_bytes());
                e[8..12].copy_from_slice(&(name.len() as u32).to_le_bytes());
                e[12..16].copy_from_slice(&(esz as u32).to_le_bytes());
                e[16..16 + name.len()].copy_from_slice(name.as_bytes());
                d.extend(e);
            }
            d
        };
        let root = dir(&[("uroot", 1, 3)]);
        let uroot = dir(&[(files[0].0, 2, 2), ("d", 3, 3)]);
        let sub = dir(&[(files[1].0, 4, 2)]);
        let mut block = first_block;
        for (n, data) in [(0usize, &root[..]), (1, &uroot[..]), (3, &sub[..]), (2, files[0].1), (4, files[1].1)] {
            put_inode(&mut img, n, data.len() as u64, block);
            let o = block as usize * bsz;
            if img.len() < o + data.len() + bsz {
                img.resize(o + data.len() + bsz, 0);
            }
            img[o..o + data.len()].copy_from_slice(data);
            block += (data.len().div_ceil(bsz)).max(1) as u32;
        }
        img
    }

    /// `data` as a PFSC image: zlib blocks, stored when that is not smaller, empty for all-zero blocks.
    pub fn pfsc(data: &[u8], bsz: usize) -> Vec<u8> {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        let blocks: Vec<Vec<u8>> = data
            .chunks(bsz)
            .map(|b| {
                if b.iter().all(|&x| x == 0) && b.len() == bsz {
                    return Vec::new();
                }
                let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
                e.write_all(b).unwrap();
                let z = e.finish().unwrap();
                if z.len() >= bsz { let mut s = b.to_vec(); s.resize(bsz, 0); s } else { z }
            })
            .collect();
        let table = 0x30usize;
        let start = (table + (blocks.len() + 1) * 8).div_ceil(16) * 16;
        let mut img = vec![0u8; start];
        img[0..4].copy_from_slice(b"PFSC");
        img[0x0C..0x10].copy_from_slice(&(bsz as u32).to_le_bytes());
        img[0x18..0x20].copy_from_slice(&(table as u64).to_le_bytes());
        img[0x20..0x28].copy_from_slice(&(start as u64).to_le_bytes());
        img[0x28..0x30].copy_from_slice(&(data.len() as u64).to_le_bytes());
        let mut pos = start as u64;
        for (i, b) in blocks.iter().enumerate() {
            img[table + i * 8..table + i * 8 + 8].copy_from_slice(&pos.to_le_bytes());
            pos += b.len() as u64;
        }
        img[table + blocks.len() * 8..table + blocks.len() * 8 + 8].copy_from_slice(&pos.to_le_bytes());
        for b in blocks {
            img.extend(b);
        }
        img
    }

    /// A fake PKG like the real ones: header, entry table with `entries` (id, bytes), outer PFS encrypted
    /// from sector 16 with the zero-passcode keys, holding `uroot/pfs_image.dat` = PFSC of an inner PFS
    /// with `files[0]` at the root and `files[1]` in `d/`.
    pub fn synthetic_pkg(content_id: &[u8; 36], entries: &[(u32, &[u8])], files: &[(&str, &[u8])]) -> Vec<u8> {
        let inner = tiny_pfs(0x1000, 4, files);
        let image = pfsc(&inner, 0x4000);
        let mut outer = tiny_pfs(0x1000, 20, &[("pfs_image.dat", &image), ("unused", b"x")]);
        let seed: Vec<u8> = (100..116).collect();
        outer[0x370..0x380].copy_from_slice(&seed);
        outer.resize(outer.len().div_ceil(0x1000) * 0x1000, 0);
        let (k1, k2) = outer_keys(content_id, &seed);
        for (i, s) in outer.chunks_mut(0x1000).enumerate() {
            if i >= 16 {
                encrypt_unit(&k1, &k2, i as u64, s);
            }
        }
        let pfs_offset = 0x10000usize;
        let mut pkg = vec![0u8; pfs_offset];
        pkg[0..4].copy_from_slice(&PKG_MAGIC.to_be_bytes());
        pkg[0x10..0x14].copy_from_slice(&(entries.len() as u32).to_be_bytes());
        pkg[0x18..0x1C].copy_from_slice(&0x2000u32.to_be_bytes());
        pkg[0x40..0x64].copy_from_slice(content_id);
        pkg[0x410..0x418].copy_from_slice(&(pfs_offset as u64).to_be_bytes());
        pkg[0x418..0x420].copy_from_slice(&(outer.len() as u64).to_be_bytes());
        let mut data_at = 0x3000usize;
        for (i, (id, bytes)) in entries.iter().enumerate() {
            let e = 0x2000 + i * 32;
            pkg[e..e + 4].copy_from_slice(&id.to_be_bytes());
            pkg[e + 16..e + 20].copy_from_slice(&(data_at as u32).to_be_bytes());
            pkg[e + 20..e + 24].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
            pkg[data_at..data_at + bytes.len()].copy_from_slice(bytes);
            data_at += bytes.len().div_ceil(16) * 16;
        }
        pkg.extend(outer);
        pkg
    }

    #[test]
    fn package_open_list_and_stream() {
        let big: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let cid = b"EP0000-CUSA11182_00-TESTTESTTESTTEST";
        let bytes = synthetic_pkg(cid, &[(0x1000, b"SFO!"), (0x1200, b"icon")], &[("eboot.bin", b"ELF"), ("big.dat", &big)]);
        let dir = std::env::temp_dir().join(format!("fzh-pfs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.pkg");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(read_param_sfo(&path).unwrap(), b"SFO!");
        let pkg = Package::open(&path).unwrap();
        assert_eq!(sce_sys_entries(&pkg.entries).iter().map(|e| e.0).collect::<Vec<_>>(), vec!["param.sfo", "icon0.png"]);
        let files = pkg.files().unwrap();
        assert_eq!(files.iter().map(|f| (f.path.as_str(), f.size)).collect::<Vec<_>>(), vec![("eboot.bin", 3), ("d/big.dat", 300_000)]);
        let cancel = AtomicBool::new(false);
        for (from, workers) in [(0u64, 1usize), (12_345, 3)] {
            let mut out = Vec::new();
            let f = &files[1];
            let done = pkg.stream(f.offset + from, f.offset + f.size, workers, &cancel, &mut |b| {
                out.extend_from_slice(b);
                Ok(())
            });
            assert!(done.unwrap());
            assert_eq!(out, big[from as usize..]);
        }
        cancel.store(true, Ordering::SeqCst);
        let f = &files[1];
        assert!(!pkg.stream(f.offset, f.offset + f.size, 2, &cancel, &mut |_| Ok(())).unwrap());
        drop(pkg);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pfs_walk_and_data() {
        let big: Vec<u8> = (0..10000u32).map(|i| (i % 253) as u8).collect();
        let img = tiny_pfs(0x1000, 4, &[("a.txt", b"hello"), ("b.bin", &big)]);
        let pfs = Pfs::new(Mem(img)).unwrap();
        let root = pfs.find(0, "uroot").unwrap();
        let files = pfs.walk(root).unwrap();
        let names: Vec<_> = files.iter().map(|(p, i)| (p.as_str(), i.size)).collect();
        assert_eq!(names, vec![("a.txt", 5), ("d/b.bin", 10000)]);
        let (off, size) = pfs.data_range(&files[1].1);
        assert_eq!(pfs.src.read_vec(off, size as usize).unwrap(), big);
        assert!(Pfs::new(Mem(vec![0u8; 0x400])).is_err());
    }

    #[test]
    fn pfsc_blocks() {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        let bsz = 0x1000usize;
        let data: Vec<u8> = (0..bsz * 3 + 100).map(|i| (i / 7 % 256) as u8).collect();
        let blocks: Vec<&[u8]> = data.chunks(bsz).collect();
        let mut stored: Vec<Vec<u8>> = Vec::new();
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(blocks[0]).unwrap();
        stored.push(e.finish().unwrap());
        stored.push(blocks[1].to_vec());
        stored.push(Vec::new());
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(blocks[3]).unwrap();
        stored.push(e.finish().unwrap());
        let mut img = vec![0u8; 0x100];
        img[0..4].copy_from_slice(b"PFSC");
        img[0x0C..0x10].copy_from_slice(&(bsz as u32).to_le_bytes());
        img[0x18..0x20].copy_from_slice(&0x30u64.to_le_bytes());
        img[0x28..0x30].copy_from_slice(&(data.len() as u64).to_le_bytes());
        let mut pos = 0x100u64;
        let mut offs = vec![pos];
        for s in &stored {
            pos += s.len() as u64;
            offs.push(pos);
        }
        for (i, o) in offs.iter().enumerate() {
            img[0x30 + i * 8..0x38 + i * 8].copy_from_slice(&o.to_le_bytes());
        }
        for s in &stored {
            img.extend_from_slice(s);
        }
        let p = parse_pfsc(&Mem(img.clone())).unwrap().unwrap();
        assert_eq!((p.bsz, p.size, p.offs.len()), (bsz as u64, data.len() as u64, 5));
        let mut expect = data.clone();
        expect[2 * bsz..3 * bsz].fill(0);
        let mut got = Vec::new();
        for i in 0..4 {
            let (a, b) = (p.offs[i] as usize, p.offs[i + 1] as usize);
            got.extend(decode_block(&img[a..b], bsz).unwrap());
        }
        assert_eq!(got, expect);
        assert!(decode_block(b"garbage!", bsz).is_err());
    }
}
