//! `.forge` archive reader (format version 25) — port of RE/tools/forge.py; format in RE/08.
//! Read-only: opens the user's own game files at runtime; nothing is copied into the project.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const MAGIC: &[u8; 9] = b"scimitar\0";
const COMPRESSED_MAGIC: u64 = 0x1004_FA99_57FB_AA33;
const FILEDATA_HEADER_SIZE: u64 = 0x1B8;

#[derive(Debug, Clone)]
pub struct ForgeEntry {
    pub index: usize,
    pub offset: u64,
    pub file_id: u32,
    pub size: u32,
    pub name: String,
}

/// One serialized engine object inside a stored file.
#[derive(Debug, Clone)]
pub struct Resource {
    pub id: u32,
    /// CRC32 of the C++ class name (e.g. 0x415D9568 = "Mesh").
    pub class_hash: u32,
    pub name: String,
    pub payload: Vec<u8>,
}

pub struct Forge {
    file: File,
    pub entries: Vec<ForgeEntry>,
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

impl Forge {
    pub fn open(path: &Path) -> io::Result<Forge> {
        let mut file = File::open(path)?;
        let mut hdr = [0u8; 0x1D];
        file.read_exact(&mut hdr)?;
        if &hdr[..9] != MAGIC {
            return Err(bad("not a forge file"));
        }
        let version = rd_u32(&hdr, 9);
        if version != 25 {
            return Err(bad(format!("unsupported forge version {version}")));
        }
        let fdh = rd_u64(&hdr, 0x0D);
        let mut forge = Forge { file, entries: Vec::new() };
        let h = forge.read_at(fdh, 0x28)?;
        let mut block = rd_u64(&h, 0x20);
        while block != 0 && block != u64::MAX {
            let b = forge.read_at(block, 0x30)?;
            let count = rd_u32(&b, 0) as i32;
            let index_off = rd_u64(&b, 8);
            let next = rd_u64(&b, 0x10);
            let names_off = rd_u64(&b, 0x20);
            if count > 0 {
                let n = count as usize;
                let idx = forge.read_at(index_off, 16 * n)?;
                let names = forge.read_at(names_off, 0xBC * n)?;
                for i in 0..n {
                    let nb = &names[0xBC * i..0xBC * (i + 1)];
                    let raw = &nb[0x2C..0xAC];
                    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                    forge.entries.push(ForgeEntry {
                        index: forge.entries.len(),
                        offset: rd_u64(&idx, 16 * i),
                        file_id: rd_u32(&idx, 16 * i + 8),
                        size: rd_u32(&idx, 16 * i + 12),
                        name: raw[..end].iter().map(|&c| c as char).collect(),
                    });
                }
            }
            block = next;
        }
        Ok(forge)
    }

    fn read_at(&mut self, off: u64, n: usize) -> io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(off))?;
        let mut v = vec![0u8; n];
        self.file.read_exact(&mut v)?;
        Ok(v)
    }

    pub fn find(&self, name: &str) -> Option<&ForgeEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Decode one compressed container (RE/08 §3); returns (bytes, end offset).
    fn container(&mut self, off: u64) -> io::Result<(Vec<u8>, u64)> {
        let h = self.read_at(off, 17)?;
        if rd_u64(&h, 0) != COMPRESSED_MAGIC {
            return Err(bad(format!("bad container magic at {off:#x}")));
        }
        let codec = h[10];
        if codec > 1 {
            return Err(bad(format!("unsupported codec {codec}")));
        }
        let n = rd_u16(&h, 15) as usize;
        let table = self.read_at(off + 17, 4 * n)?;
        let chunks: Vec<(usize, usize)> =
            (0..n).map(|i| (rd_u16(&table, 4 * i) as usize, rd_u16(&table, 4 * i + 2) as usize)).collect();
        let body_len: usize = chunks.iter().map(|c| c.1 + 4).sum();
        let pos = off + 17 + 4 * n as u64;
        let body = self.read_at(pos, body_len)?;
        let mut out = Vec::with_capacity(chunks.iter().map(|c| c.0).sum());
        let mut q = 0;
        for (raw, packed) in chunks {
            let chk = rd_u32(&body, q);
            let blob = &body[q + 4..q + 4 + packed];
            q += 4 + packed;
            if adler32_init0(blob) != chk {
                return Err(bad("chunk checksum mismatch"));
            }
            if packed == raw {
                out.extend_from_slice(blob);
            } else {
                let d = lzo1x_decompress(blob, raw).map_err(bad)?;
                out.extend_from_slice(&d);
            }
        }
        Ok((out, pos + q as u64))
    }

    /// All resources of a stored file (empty for uncompressed records such as GlobalMetaFile).
    pub fn resources(&mut self, entry: &ForgeEntry) -> io::Result<Vec<Resource>> {
        let start = entry.offset + FILEDATA_HEADER_SIZE;
        if rd_u64(&self.read_at(start, 8)?, 0) != COMPRESSED_MAGIC {
            return Ok(Vec::new());
        }
        let (toc, next) = self.container(start)?;
        let (data, _) = self.container(next)?;
        let n = rd_u16(&toc, 0) as usize;
        let mut out = Vec::with_capacity(n);
        let mut pos = 0usize;
        for i in 0..n {
            let id = rd_u32(&toc, 2 + 8 * i);
            let size = rd_u32(&toc, 2 + 8 * i + 4) as usize;
            let class_hash = rd_u32(&data, pos);
            let dsz = rd_u32(&data, pos + 4) as usize;
            let nl = rd_u32(&data, pos + 8) as usize;
            let name = data[pos + 12..pos + 12 + nl].iter().map(|&c| c as char).collect();
            let hdr = size - dsz; // payload = last dataSize bytes (optional sub-header before it)
            out.push(Resource { id, class_hash, name, payload: data[pos + hdr..pos + size].to_vec() });
            pos += size;
        }
        if pos != data.len() {
            return Err(bad("TOC sizes do not match data"));
        }
        Ok(out)
    }
}

/// Adler-32 with initial value 0 (the container chunk checksum, RE/08 §3).
pub fn adler32_init0(data: &[u8]) -> u32 {
    let (mut a, mut b) = (0u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// Clean-room LZO1X decompressor (public bitstream format; the game uses lzo1x_decompress 0x9A0F40).
pub fn lzo1x_decompress(src: &[u8], out_len: usize) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(out_len);
    let mut ip = 0usize;
    let get = |ip: &mut usize| -> Result<usize, String> {
        let b = *src.get(*ip).ok_or("input overrun")? as usize;
        *ip += 1;
        Ok(b)
    };
    let run_len = |t: usize, mask: usize, ip: &mut usize| -> Result<usize, String> {
        if t != 0 {
            return Ok(t);
        }
        let mut t = mask;
        while *src.get(*ip).ok_or("input overrun")? == 0 {
            t += 255;
            *ip += 1;
        }
        t += *src.get(*ip).ok_or("input overrun")? as usize;
        *ip += 1;
        Ok(t)
    };
    let copy_lit = |out: &mut Vec<u8>, ip: &mut usize, n: usize| -> Result<(), String> {
        let s = src.get(*ip..*ip + n).ok_or("literal overrun")?;
        out.extend_from_slice(s);
        *ip += n;
        Ok(())
    };
    let copy_match = |out: &mut Vec<u8>, dist: usize, n: usize| -> Result<(), String> {
        let start = out.len().checked_sub(dist).ok_or("lookbehind overrun")?;
        for k in 0..n {
            let b = out[start + k];
            out.push(b);
        }
        Ok(())
    };

    let mut state = 0usize;
    let mut t = get(&mut ip)?;
    if t > 17 {
        copy_lit(&mut out, &mut ip, t - 17)?;
        state = if t - 17 >= 4 { 4 } else { t - 17 };
        t = get(&mut ip)?;
    }
    loop {
        let (cnt, dist);
        if t >= 64 {
            cnt = (t >> 5) - 1 + 2;
            dist = ((t >> 2) & 7) + (get(&mut ip)? << 3) + 1;
        } else if t >= 32 {
            cnt = run_len(t & 31, 31, &mut ip)? + 2;
            let lo = get(&mut ip)?;
            let hi = get(&mut ip)?;
            dist = ((hi << 8 | lo) >> 2) + 1;
            t = lo;
        } else if t >= 16 {
            cnt = run_len(t & 7, 7, &mut ip)? + 2;
            let lo = get(&mut ip)?;
            let hi = get(&mut ip)?;
            let d = ((t & 8) << 11) + ((hi << 8 | lo) >> 2);
            if d == 0 {
                break;
            }
            dist = d + 16384;
            t = lo;
        } else if state == 0 {
            let n = run_len(t, 15, &mut ip)? + 3;
            copy_lit(&mut out, &mut ip, n)?;
            state = 4;
            t = get(&mut ip)?;
            continue;
        } else if state < 4 {
            cnt = 2;
            dist = (t >> 2) + (get(&mut ip)? << 2) + 1;
        } else {
            cnt = 3;
            dist = (t >> 2) + (get(&mut ip)? << 2) + 2049;
        }
        copy_match(&mut out, dist, cnt)?;
        let lit = t & 3;
        copy_lit(&mut out, &mut ip, lit)?;
        state = lit;
        t = get(&mut ip)?;
    }
    if out.len() != out_len {
        return Err(format!("size mismatch {} != {}", out.len(), out_len));
    }
    Ok(out)
}

/// CRC32 (zlib/IEEE) — the hash used for class and property names (RE/07 §4b).
pub fn crc32(s: &str) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in s.as_bytes() {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}
