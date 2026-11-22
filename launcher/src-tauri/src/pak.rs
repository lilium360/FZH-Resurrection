//! Unreal Engine 4 .pak, version 4: reader for uncompressed, unencrypted entries
//! and writer (reference implementation: `tools/modpak.py`).

use sha1::{Digest, Sha1};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const MAGIC: u32 = 0x5A6F12E1;
pub const VERSION: i32 = 4;
const FOOTER_LEN: u64 = 45;
const MOUNT_POINT: &str = "../../../";

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub offset: i64,
    pub size: i64,
    pub method: i32,
    pub encrypted: bool,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("truncated pak index")?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn fstring(&mut self) -> Result<String, String> {
        let n = self.i32()?;
        if n >= 0 {
            let b = self.take(n as usize)?;
            Ok(String::from_utf8_lossy(b).trim_end_matches('\0').to_string())
        } else {
            let b = self.take(n.unsigned_abs() as usize * 2)?;
            let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            Ok(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
        }
    }
}

pub fn read_index(f: &mut File) -> Result<Vec<Entry>, String> {
    let size = f.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    if size < FOOTER_LEN {
        return Err("file too small to be a pak".into());
    }
    f.seek(SeekFrom::Start(size - FOOTER_LEN)).map_err(|e| e.to_string())?;
    let mut tail = [0u8; FOOTER_LEN as usize];
    f.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let magic = u32::from_le_bytes(tail[1..5].try_into().unwrap());
    let version = u32::from_le_bytes(tail[5..9].try_into().unwrap());
    let index_offset = u64::from_le_bytes(tail[9..17].try_into().unwrap());
    let index_size = u64::from_le_bytes(tail[17..25].try_into().unwrap());
    if magic != MAGIC {
        return Err("not an Unreal Engine pak".into());
    }
    if tail[0] != 0 {
        return Err("the pak index is encrypted".into());
    }
    if !(1..=4).contains(&version) {
        return Err(format!("unsupported pak version {version}"));
    }
    if index_offset.checked_add(index_size).is_none_or(|end| end > size) || index_size > 512 << 20 {
        return Err("bad pak index position".into());
    }
    f.seek(SeekFrom::Start(index_offset)).map_err(|e| e.to_string())?;
    let mut index = vec![0u8; index_size as usize];
    f.read_exact(&mut index).map_err(|e| e.to_string())?;
    let mut c = Cursor { data: &index, pos: 0 };
    c.fstring()?;
    let count = c.i32()?;
    let mut entries = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count {
        let name = c.fstring()?;
        let offset = c.i64()?;
        let size = c.i64()?;
        let _uncompressed = c.i64()?;
        let method = c.i32()?;
        c.take(20)?;
        if method != 0 {
            let blocks = c.i32()?.max(0) as usize;
            c.take(blocks * 16)?;
        }
        let encrypted = c.u8()? != 0;
        c.take(4)?;
        entries.push(Entry { name, offset, size, method, encrypted });
    }
    Ok(entries)
}

pub fn entry_data(f: &mut File, e: &Entry) -> Result<Vec<u8>, String> {
    if e.encrypted {
        return Err(format!("{} is encrypted", e.name));
    }
    if e.method != 0 {
        return Err(format!("{} is compressed", e.name));
    }
    let header = 48 + 5;
    f.seek(SeekFrom::Start((e.offset + header) as u64)).map_err(|e| e.to_string())?;
    let mut out = vec![0u8; e.size.max(0) as usize];
    f.read_exact(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Reads the named entries of a pak; fails if one is missing.
pub fn read_files(path: &Path, names: &[&str]) -> Result<Vec<Vec<u8>>, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let entries = read_index(&mut f)?;
    names
        .iter()
        .map(|n| {
            let e = entries.iter().find(|e| e.name == *n).ok_or_else(|| format!("{n} not found in the game pak"))?;
            entry_data(&mut f, e)
        })
        .collect()
}

fn fstring(s: &str) -> Vec<u8> {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    let mut out = (b.len() as i32).to_le_bytes().to_vec();
    out.extend_from_slice(&b);
    out
}

fn entry_record(offset: i64, data: &[u8], sha1: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(53);
    r.extend_from_slice(&offset.to_le_bytes());
    r.extend_from_slice(&(data.len() as i64).to_le_bytes());
    r.extend_from_slice(&(data.len() as i64).to_le_bytes());
    r.extend_from_slice(&0i32.to_le_bytes());
    r.extend_from_slice(sha1);
    r.push(0);
    r.extend_from_slice(&0u32.to_le_bytes());
    r
}

/// Builds a pak from (path inside the pak, contents), sorted by path.
pub fn write(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut body = Vec::new();
    let mut index_entries = Vec::new();
    for (name, data) in sorted.iter().map(|f| (&f.0, &f.1)) {
        let sha1 = Sha1::digest(data);
        let offset = body.len() as i64;
        body.extend_from_slice(&entry_record(0, data, &sha1));
        body.extend_from_slice(data);
        index_entries.extend_from_slice(&fstring(name));
        index_entries.extend_from_slice(&entry_record(offset, data, &sha1));
    }
    let mut index = fstring(MOUNT_POINT);
    index.extend_from_slice(&(sorted.len() as i32).to_le_bytes());
    index.extend_from_slice(&index_entries);
    let mut out = body;
    let body_len = out.len() as i64;
    out.extend_from_slice(&index);
    out.push(0);
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&body_len.to_le_bytes());
    out.extend_from_slice(&(index.len() as i64).to_le_bytes());
    out.extend_from_slice(&Sha1::digest(&index));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn write_then_read() {
        let files = vec![
            ("firewall/Config/PS4/PS4Engine.ini".to_string(), b"[A]\r\nb=1\r\n".to_vec()),
            ("firewall/Config/DefaultEngine.ini".to_string(), b"x=y".to_vec()),
        ];
        let pak = write(&files);
        let dir = std::env::temp_dir().join(format!("fzh-pak-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.pak");
        File::create(&path).unwrap().write_all(&pak).unwrap();
        let mut f = File::open(&path).unwrap();
        let entries = read_index(&mut f).unwrap();
        assert_eq!(entries[0].name, "firewall/Config/DefaultEngine.ini");
        assert_eq!(entries[1].name, "firewall/Config/PS4/PS4Engine.ini");
        let got = read_files(&path, &["firewall/Config/PS4/PS4Engine.ini", "firewall/Config/DefaultEngine.ini"]).unwrap();
        assert_eq!(got[0], b"[A]\r\nb=1\r\n");
        assert_eq!(got[1], b"x=y");
        drop(f);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    /// Golden file written by `python tools/modpak.py pack tests/fixtures/pak-src reference.pak`.
    #[test]
    fn byte_identical_to_the_reference() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
        let golden = std::fs::read(root.join("reference.pak")).unwrap();
        let src = root.join("pak-src");
        let files = vec![
            (
                "firewall/Config/DefaultEngine.ini".to_string(),
                std::fs::read(src.join("firewall").join("Config").join("DefaultEngine.ini")).unwrap(),
            ),
            (
                "firewall/Config/PS4/PS4Engine.ini".to_string(),
                std::fs::read(src.join("firewall").join("Config").join("PS4").join("PS4Engine.ini")).unwrap(),
            ),
        ];
        assert_eq!(write(&files), golden);
    }
}
