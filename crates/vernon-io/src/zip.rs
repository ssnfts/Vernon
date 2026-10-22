//! Minimal ZIP container for 3MF packages.
//!
//! Reader: stored and deflate entries, ZIP64 end records and extra fields,
//! CRC-32 verification, and hard caps on entry count and decompressed size so
//! a crafted archive cannot exhaust memory. Writer: deflate (or stored when
//! that is smaller), fixed timestamps so output is reproducible, no ZIP64.

use std::cell::Cell;
use std::collections::HashMap;

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::decompress_to_vec_with_limit;

use crate::{parse_err, IoError, IoResult};

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_ZIP64_EOCD: u32 = 0x0606_4b50;
const SIG_ZIP64_LOCATOR: u32 = 0x0706_4b50;

/// Caps applied while reading an archive.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub max_entries: usize,
    /// Largest decompressed size accepted for one entry.
    pub max_entry_bytes: u64,
    /// Largest total of all entries actually decompressed.
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 65_536,
            max_entry_bytes: 1 << 30,
            max_total_bytes: 2 << 30,
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    method: u16,
    flags: u16,
    crc: u32,
    comp: u64,
    uncomp: u64,
    local_offset: u64,
}

/// A parsed archive held in memory. Entries are inflated on demand.
pub(crate) struct Archive<'a> {
    data: &'a [u8],
    file: &'a str,
    entries: Vec<Entry>,
    /// Lower-cased normalised name -> entry index (OPC part names are
    /// case-insensitive).
    index: HashMap<String, usize>,
    limits: Limits,
    inflated: Cell<u64>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

/// Normalise a part name: forward slashes, no leading slash.
pub(crate) fn normalize(name: &str) -> String {
    let n = name.replace('\\', "/");
    n.trim_start_matches('/').trim_start_matches("./").to_string()
}

impl<'a> Archive<'a> {
    pub(crate) fn open(data: &'a [u8], file: &'a str, limits: Limits) -> IoResult<Self> {
        let bad = |m: &str| parse_err(file, 0, format!("bad ZIP archive: {m}"));
        if data.len() < 22 {
            return Err(bad("file is shorter than an end-of-central-directory record"));
        }
        // The end record sits at the very end, optionally followed by a comment.
        let lo = data.len().saturating_sub(22 + 65_535);
        let eocd = (lo..=data.len() - 22)
            .rev()
            .find(|&i| u32_at(data, i) == Some(SIG_EOCD))
            .ok_or_else(|| bad("no end-of-central-directory record"))?;
        let mut total = u16_at(data, eocd + 10).unwrap_or(0) as u64;
        let mut cd_size = u32_at(data, eocd + 12).unwrap_or(0) as u64;
        let mut cd_off = u32_at(data, eocd + 16).unwrap_or(0) as u64;
        if total == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF {
            // ZIP64: the locator sits 20 bytes before the end record.
            let loc = eocd
                .checked_sub(20)
                .filter(|&l| u32_at(data, l) == Some(SIG_ZIP64_LOCATOR))
                .ok_or_else(|| bad("ZIP64 locator missing"))?;
            let rec = u64_at(data, loc + 8).ok_or_else(|| bad("truncated ZIP64 locator"))? as usize;
            if u32_at(data, rec) != Some(SIG_ZIP64_EOCD) {
                return Err(bad("ZIP64 end record missing"));
            }
            total = u64_at(data, rec + 32).ok_or_else(|| bad("truncated ZIP64 end record"))?;
            cd_size = u64_at(data, rec + 40).ok_or_else(|| bad("truncated ZIP64 end record"))?;
            cd_off = u64_at(data, rec + 48).ok_or_else(|| bad("truncated ZIP64 end record"))?;
        }
        if total > limits.max_entries as u64 {
            return Err(bad(&format!(
                "{total} entries exceed the limit of {}",
                limits.max_entries
            )));
        }
        // Every central entry is at least 46 bytes; check before allocating.
        if total.saturating_mul(46) > data.len() as u64 {
            return Err(bad("entry count exceeds the archive size"));
        }
        if cd_off.checked_add(cd_size).is_none_or(|e| e > data.len() as u64) {
            return Err(bad("central directory lies outside the file"));
        }

        let mut entries = Vec::with_capacity(total as usize);
        let mut index = HashMap::new();
        let mut pos = cd_off as usize;
        for _ in 0..total {
            if u32_at(data, pos) != Some(SIG_CENTRAL) {
                return Err(bad("bad central directory entry"));
            }
            let g16 = |o: usize| u16_at(data, pos + o).ok_or_else(|| bad("truncated central entry"));
            let g32 = |o: usize| u32_at(data, pos + o).ok_or_else(|| bad("truncated central entry"));
            let flags = g16(8)?;
            let method = g16(10)?;
            let crc = g32(16)?;
            let mut comp = g32(20)? as u64;
            let mut uncomp = g32(24)? as u64;
            let name_len = g16(28)? as usize;
            let extra_len = g16(30)? as usize;
            let comment_len = g16(32)? as usize;
            let mut local_offset = g32(42)? as u64;
            let name_bytes = data
                .get(pos + 46..pos + 46 + name_len)
                .ok_or_else(|| bad("truncated entry name"))?;
            let extra = data
                .get(pos + 46 + name_len..pos + 46 + name_len + extra_len)
                .ok_or_else(|| bad("truncated extra field"))?;
            // ZIP64 extra (id 1): only fields whose 32-bit value is saturated.
            let mut e = 0;
            while e + 4 <= extra.len() {
                let id = u16_at(extra, e).unwrap_or(0);
                let sz = u16_at(extra, e + 2).unwrap_or(0) as usize;
                let body = extra.get(e + 4..e + 4 + sz).ok_or_else(|| bad("bad extra field"))?;
                if id == 1 {
                    let mut o = 0;
                    let mut take = |v: &mut u64, sat: bool| -> IoResult<()> {
                        if sat {
                            *v = u64_at(body, o).ok_or_else(|| bad("short ZIP64 extra field"))?;
                            o += 8;
                        }
                        Ok(())
                    };
                    take(&mut uncomp, uncomp == 0xFFFF_FFFF)?;
                    take(&mut comp, comp == 0xFFFF_FFFF)?;
                    take(&mut local_offset, local_offset == 0xFFFF_FFFF)?;
                }
                e += 4 + sz;
            }
            pos += 46 + name_len + extra_len + comment_len;
            let name = normalize(&String::from_utf8_lossy(name_bytes));
            if name.ends_with('/') {
                continue; // directory entry
            }
            index.entry(name.to_ascii_lowercase()).or_insert(entries.len());
            entries.push(Entry {
                name,
                method,
                flags,
                crc,
                comp,
                uncomp,
                local_offset,
            });
        }
        Ok(Self {
            data,
            file,
            entries,
            index,
            limits,
            inflated: Cell::new(0),
        })
    }

    /// Entry names in archive order.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.index.contains_key(&normalize(name).to_ascii_lowercase())
    }

    /// Inflate the named entry (case-insensitive), or `None` if absent.
    pub(crate) fn read(&self, name: &str) -> IoResult<Option<Vec<u8>>> {
        match self.index.get(&normalize(name).to_ascii_lowercase()) {
            Some(&i) => self.inflate(&self.entries[i]).map(Some),
            None => Ok(None),
        }
    }

    fn inflate(&self, e: &Entry) -> IoResult<Vec<u8>> {
        let bad = |m: String| parse_err(self.file, 0, format!("ZIP entry '{}': {m}", e.name));
        if e.flags & 1 != 0 {
            return Err(bad("encrypted entries are not supported".into()));
        }
        if e.uncomp > self.limits.max_entry_bytes {
            return Err(bad(format!(
                "declares {} bytes, over the {} byte limit",
                e.uncomp, self.limits.max_entry_bytes
            )));
        }
        let total = self.inflated.get().saturating_add(e.uncomp);
        if total > self.limits.max_total_bytes {
            return Err(bad(format!(
                "archive would inflate past {} bytes in total",
                self.limits.max_total_bytes
            )));
        }
        let lo = e.local_offset as usize;
        if u32_at(self.data, lo) != Some(SIG_LOCAL) {
            return Err(bad("bad local header".into()));
        }
        let name_len = u16_at(self.data, lo + 26).ok_or_else(|| bad("truncated local header".into()))?;
        let extra_len = u16_at(self.data, lo + 28).ok_or_else(|| bad("truncated local header".into()))?;
        let start = lo + 30 + name_len as usize + extra_len as usize;
        let end = start
            .checked_add(e.comp as usize)
            .filter(|&end| e.comp <= self.data.len() as u64 && end <= self.data.len())
            .ok_or_else(|| bad("data lies outside the file".into()))?;
        let raw = &self.data[start..end];
        let out = match e.method {
            0 => {
                if e.comp != e.uncomp {
                    return Err(bad("stored entry with different sizes".into()));
                }
                raw.to_vec()
            }
            8 => decompress_to_vec_with_limit(raw, e.uncomp as usize)
                .map_err(|err| bad(format!("corrupt or oversized deflate stream ({:?})", err.status)))?,
            m => return Err(bad(format!("unsupported compression method {m}"))),
        };
        if out.len() as u64 != e.uncomp {
            return Err(bad(format!(
                "inflated to {} bytes, header says {}",
                out.len(),
                e.uncomp
            )));
        }
        if crc32(&out) != e.crc {
            return Err(bad("CRC-32 mismatch".into()));
        }
        self.inflated.set(total);
        Ok(out)
    }
}

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 (IEEE) as used by ZIP.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

struct Central {
    name: String,
    method: u16,
    crc: u32,
    comp: u32,
    uncomp: u32,
    offset: u32,
}

/// Builds an archive in memory.
#[derive(Default)]
pub(crate) struct ZipWriter {
    buf: Vec<u8>,
    central: Vec<Central>,
}

/// 1980-01-01 00:00:00 in DOS format; fixed so output is reproducible.
const DOS_DATE: u16 = 0x0021;

impl ZipWriter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add(&mut self, name: &str, data: &[u8]) -> IoResult<()> {
        let too_big = || IoError::Other("archive too large for the 3MF writer (no ZIP64)".into());
        if self.central.len() >= 0xFFFE {
            return Err(too_big());
        }
        let uncomp = u32::try_from(data.len()).map_err(|_| too_big())?;
        let packed = compress_to_vec(data, 6);
        let (method, body): (u16, &[u8]) = if packed.len() < data.len() {
            (8, &packed)
        } else {
            (0, data)
        };
        let comp = u32::try_from(body.len()).map_err(|_| too_big())?;
        let offset = u32::try_from(self.buf.len()).map_err(|_| too_big())?;
        let crc = crc32(data);
        let b = &mut self.buf;
        b.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        b.extend_from_slice(&(if method == 8 { 20u16 } else { 10 }).to_le_bytes());
        b.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        b.extend_from_slice(&method.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // time
        b.extend_from_slice(&DOS_DATE.to_le_bytes());
        b.extend_from_slice(&crc.to_le_bytes());
        b.extend_from_slice(&comp.to_le_bytes());
        b.extend_from_slice(&uncomp.to_le_bytes());
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // extra length
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(body);
        self.central.push(Central {
            name: name.to_string(),
            method,
            crc,
            comp,
            uncomp,
            offset,
        });
        Ok(())
    }

    pub(crate) fn finish(mut self) -> IoResult<Vec<u8>> {
        let too_big = || IoError::Other("archive too large for the 3MF writer (no ZIP64)".into());
        let cd_start = u32::try_from(self.buf.len()).map_err(|_| too_big())?;
        let mut cd = Vec::new();
        for c in &self.central {
            cd.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
            cd.extend_from_slice(&20u16.to_le_bytes()); // version made by
            cd.extend_from_slice(&(if c.method == 8 { 20u16 } else { 10 }).to_le_bytes());
            cd.extend_from_slice(&0x0800u16.to_le_bytes());
            cd.extend_from_slice(&c.method.to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes());
            cd.extend_from_slice(&DOS_DATE.to_le_bytes());
            cd.extend_from_slice(&c.crc.to_le_bytes());
            cd.extend_from_slice(&c.comp.to_le_bytes());
            cd.extend_from_slice(&c.uncomp.to_le_bytes());
            cd.extend_from_slice(&(c.name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes()); // extra
            cd.extend_from_slice(&0u16.to_le_bytes()); // comment
            cd.extend_from_slice(&0u16.to_le_bytes()); // disk
            cd.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            cd.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            cd.extend_from_slice(&c.offset.to_le_bytes());
            cd.extend_from_slice(c.name.as_bytes());
        }
        let cd_size = u32::try_from(cd.len()).map_err(|_| too_big())?;
        self.buf.extend_from_slice(&cd);
        let n = self.central.len() as u16;
        let b = &mut self.buf;
        b.extend_from_slice(&SIG_EOCD.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
        b.extend_from_slice(&cd_size.to_le_bytes());
        b.extend_from_slice(&cd_start.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        Ok(self.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut z = ZipWriter::new();
        z.add("[Content_Types].xml", b"<Types/>").unwrap();
        z.add("3D/big.bin", &vec![7u8; 100_000]).unwrap();
        z.add("noise.bin", &(0..=255u8).cycle().take(3000).collect::<Vec<_>>())
            .unwrap();
        z.finish().unwrap()
    }

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn round_trip_stored_and_deflated() {
        let bytes = sample();
        assert_eq!(&bytes[..4], &[0x50, 0x4B, 0x03, 0x04]);
        let a = Archive::open(&bytes, "t.zip", Limits::default()).unwrap();
        assert_eq!(a.names().count(), 3);
        assert_eq!(a.read("[content_types].XML").unwrap().unwrap(), b"<Types/>");
        assert_eq!(a.read("/3D\\big.bin").unwrap().unwrap(), vec![7u8; 100_000]);
        assert_eq!(a.read("noise.bin").unwrap().unwrap().len(), 3000);
        assert!(a.read("missing").unwrap().is_none());
        // Compressible data must actually be deflated.
        assert!(bytes.len() < 20_000);
    }

    #[test]
    fn caps_stop_oversized_entries_before_allocating() {
        let bytes = sample();
        let tight = Limits {
            max_entry_bytes: 50_000,
            ..Limits::default()
        };
        let a = Archive::open(&bytes, "t.zip", tight).unwrap();
        assert!(a.read("3D/big.bin").is_err());
        assert!(a.read("[Content_Types].xml").unwrap().is_some());
        let few = Limits {
            max_entries: 2,
            ..Limits::default()
        };
        assert!(Archive::open(&bytes, "t.zip", few).is_err());
        let total = Limits {
            max_total_bytes: 101_000,
            ..Limits::default()
        };
        let a = Archive::open(&bytes, "t.zip", total).unwrap();
        assert!(a.read("3D/big.bin").unwrap().is_some());
        assert!(a.read("noise.bin").is_err());
    }

    #[test]
    fn lying_headers_and_corruption_are_errors() {
        let mut bytes = sample();
        // Declared size far above the real one (central directory field).
        let cd = (0..bytes.len() - 4)
            .find(|&i| u32_at(&bytes, i) == Some(SIG_CENTRAL))
            .unwrap();
        let mut lie = bytes.clone();
        lie[cd + 24..cd + 28].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        let a = Archive::open(&lie, "t.zip", Limits::default()).unwrap();
        assert!(a.read("[Content_Types].xml").is_err());
        // Declared size below the real one: the inflate limit stops it.
        let big = (cd..bytes.len() - 4)
            .find(|&i| u32_at(&bytes, i) == Some(SIG_CENTRAL) && i > cd)
            .unwrap();
        let mut small = bytes.clone();
        small[big + 24..big + 28].copy_from_slice(&10u32.to_le_bytes());
        let a = Archive::open(&small, "t.zip", Limits::default()).unwrap();
        assert!(a.read("3D/big.bin").is_err());
        // Flipped payload byte: CRC or deflate error.
        let local = 30 + "[Content_Types].xml".len();
        bytes[local + 2] ^= 0xFF;
        let a = Archive::open(&bytes, "t.zip", Limits::default()).unwrap();
        assert!(a.read("[Content_Types].xml").is_err());
    }

    #[test]
    fn every_truncation_is_handled() {
        let bytes = sample();
        for n in 0..bytes.len().min(400) {
            let _ = Archive::open(&bytes[..n], "t.zip", Limits::default());
        }
        for n in (0..bytes.len()).step_by(97) {
            if let Ok(a) = Archive::open(&bytes[..n], "t.zip", Limits::default()) {
                let names: Vec<String> = a.names().map(str::to_string).collect();
                for name in names {
                    let _ = a.read(&name);
                }
            }
        }
        assert!(Archive::open(b"not a zip at all, but long enough to scan", "t", Limits::default()).is_err());
    }

    #[test]
    fn zip64_end_records_are_followed() {
        // Hand-built ZIP64 archive: one stored entry whose sizes live in the extra field.
        let payload = b"hello zip64";
        let name = b"a.txt";
        let crc = crc32(payload);
        let mut b = Vec::new();
        b.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        b.extend_from_slice(&[45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        b.extend_from_slice(&crc.to_le_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(name);
        b.extend_from_slice(payload);
        let cd_start = b.len() as u64;
        let mut extra = Vec::new();
        extra.extend_from_slice(&1u16.to_le_bytes());
        extra.extend_from_slice(&16u16.to_le_bytes());
        extra.extend_from_slice(&(payload.len() as u64).to_le_bytes()); // uncompressed
        extra.extend_from_slice(&(payload.len() as u64).to_le_bytes()); // compressed
        b.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        b.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        b.extend_from_slice(&crc.to_le_bytes());
        b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        b.extend_from_slice(&[0; 12]);
        b.extend_from_slice(&0u32.to_le_bytes()); // local offset 0
        b.extend_from_slice(name);
        b.extend_from_slice(&extra);
        let cd_size = b.len() as u64 - cd_start;
        let rec = b.len() as u64;
        b.extend_from_slice(&SIG_ZIP64_EOCD.to_le_bytes());
        b.extend_from_slice(&44u64.to_le_bytes());
        b.extend_from_slice(&[45, 0, 45, 0]);
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&cd_size.to_le_bytes());
        b.extend_from_slice(&cd_start.to_le_bytes());
        b.extend_from_slice(&SIG_ZIP64_LOCATOR.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&rec.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&SIG_EOCD.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&0xFFFFu16.to_le_bytes());
        b.extend_from_slice(&0xFFFFu16.to_le_bytes());
        b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        let a = Archive::open(&b, "z64.zip", Limits::default()).unwrap();
        assert_eq!(a.read("a.txt").unwrap().unwrap(), payload);
    }
}
