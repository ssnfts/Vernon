//! E57 (ASTM E2807) point-cloud reader.
//!
//! Written from the public format description. The file is a sequence of
//! 1024-byte pages, each holding 1020 payload bytes and a CRC-32C of them
//! (stored big-endian). The 48-byte file header sits at the start of the first
//! payload, binary sections hold the point data, and an XML section describes
//! the scans. Section offsets in the header and the XML are physical (they
//! count the checksum bytes); lengths inside sections are logical.
//!
//! What is read: every `data3D` scan whose points are a `CompressedVector`
//! with `cartesianX/Y/Z`, plus `colorRed/Green/Blue` (-> `Cd`, 0..1),
//! `intensity` (as stored) and `cartesianInvalidState` (points with a non-zero
//! state are dropped). Prototype fields may be `Integer`, `ScaledInteger`
//! (bit-packed, width from the declared range) or `Float` (single or double).
//! Each scan's pose (unit quaternion plus translation) is applied, and the
//! result is converted from metres, the unit E57 always uses, to millimetres.
//!
//! A kept point whose intensity or colour value is NaN or infinite (possible
//! in float fields) is an error, so no non-finite attribute reaches the
//! geometry; points dropped as invalid are not examined.
//!
//! Everything else is an explicit error, never a guess: other encodings,
//! custom codecs, nested prototypes, spherical-only scans, compressor
//! restarts, a bad checksum, a length that disagrees with the header, and any
//! offset or count that does not fit inside the file. Declared record counts
//! are checked against the bytes actually present before anything is
//! allocated, and decoded columns grow only as real data is consumed.

use glam::{DQuat, DVec3};
use vernon_core::{AttribArray, Geometry};

use crate::bin::{u16_at, u32_at, u64_at, u8_at};
use crate::xml::{self, Event};
use crate::{parse_err, IoResult};

type R<T> = Result<T, String>;

const PAGE: usize = 1024;
const PAYLOAD: usize = 1020;
const FILE_HEADER: usize = 48;
const SIGNATURE: &[u8; 8] = b"ASTM-E57";
const SECTION_HEADER: u64 = 32;
/// Largest record count accepted when no field stores any bits (all constant).
const MAX_CONSTANT_RECORDS: u64 = 1 << 16;
/// E57 lengths are metres; Vernon works in millimetres.
const M_TO_MM: f64 = 1000.0;

pub(crate) fn read(bytes: &[u8], file: &str) -> IoResult<Geometry> {
    read_inner(bytes).map_err(|m| parse_err(file, 0, format!("E57: {m}")))
}

fn read_inner(bytes: &[u8]) -> R<Geometry> {
    let paged = Paged::open(bytes)?;
    let xml_at = paged.to_logical(u64_at(bytes, 24)?)?;
    let xml_len = u64_at(bytes, 32)?;
    let mut raw = Vec::new();
    paged.read_into(xml_at, xml_len, &mut raw)?;
    let text = std::str::from_utf8(&raw).map_err(|_| "the XML section is not valid UTF-8")?;
    let scans = parse_xml(text)?;
    if scans.is_empty() {
        return Err("the file contains no data3D scans".into());
    }
    let mut decoded = Vec::with_capacity(scans.len());
    for (i, s) in scans.iter().enumerate() {
        decoded.push(decode_scan(&paged, s).map_err(|m| format!("scan {}: {m}", i + 1))?);
    }
    let total: usize = decoded.iter().map(|d| d.points.len()).sum();
    let with_points = || decoded.iter().filter(|d| !d.points.is_empty());
    let all_colour = with_points().all(|d| d.colors.is_some());
    let all_intensity = with_points().all(|d| d.intensity.is_some());
    let mut points = Vec::with_capacity(total);
    let mut colors = Vec::new();
    let mut intensity = Vec::new();
    for d in decoded {
        points.extend(d.points);
        if all_colour {
            colors.extend(d.colors.unwrap_or_default());
        }
        if all_intensity {
            intensity.extend(d.intensity.unwrap_or_default());
        }
    }
    let mut geo = Geometry::from_points(points);
    if total > 0 && all_colour {
        geo.point_attribs.set("Cd", AttribArray::Vec3(colors));
    }
    if total > 0 && all_intensity {
        geo.point_attribs
            .set("intensity", AttribArray::Float(intensity));
    }
    Ok(geo)
}

// ---------------------------------------------------------------- checksum

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0x82F6_3B78 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32C (Castagnoli), the checksum E57 puts on every page.
pub(crate) fn crc32c(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

// ------------------------------------------------------------ paged access

/// The file with every page checksum verified, addressed by logical offset
/// (the payload bytes only, checksums skipped).
struct Paged<'a> {
    bytes: &'a [u8],
    logical_len: u64,
}

impl<'a> Paged<'a> {
    fn open(bytes: &'a [u8]) -> R<Self> {
        if bytes.len() < FILE_HEADER || &bytes[..8] != SIGNATURE {
            return Err("not an E57 file (the ASTM-E57 signature is missing)".into());
        }
        if !bytes.len().is_multiple_of(PAGE) {
            return Err(format!(
                "the file is {} bytes, not a whole number of {PAGE}-byte pages: it is truncated or damaged",
                bytes.len()
            ));
        }
        // The length is a whole number of pages, so no remainder is left over.
        for (i, page) in bytes.as_chunks::<PAGE>().0.iter().enumerate() {
            let stored =
                u32::from_be_bytes([page[PAYLOAD], page[PAYLOAD + 1], page[PAYLOAD + 2], page[PAYLOAD + 3]]);
            if crc32c(&page[..PAYLOAD]) != stored {
                return Err(format!(
                    "checksum mismatch in page {i} (file offset {}): the file is corrupt",
                    i * PAGE
                ));
            }
        }
        let (major, minor) = (u32_at(bytes, 8)?, u32_at(bytes, 12)?);
        if major != 1 {
            return Err(format!("unsupported E57 version {major}.{minor}"));
        }
        let declared = u64_at(bytes, 16)?;
        if declared != bytes.len() as u64 {
            return Err(format!(
                "the header declares {declared} bytes but the file has {}: it is truncated or has trailing data",
                bytes.len()
            ));
        }
        let page_size = u64_at(bytes, 40)?;
        if page_size != PAGE as u64 {
            return Err(format!("unsupported page size {page_size}"));
        }
        Ok(Self {
            bytes,
            logical_len: (bytes.len() / PAGE * PAYLOAD) as u64,
        })
    }

    /// Physical file offset to logical payload offset.
    fn to_logical(&self, phys: u64) -> R<u64> {
        let (page, within) = (phys / PAGE as u64, phys % PAGE as u64);
        if within >= PAYLOAD as u64 {
            return Err(format!("offset {phys} points into a page checksum"));
        }
        page.checked_mul(PAYLOAD as u64)
            .and_then(|l| l.checked_add(within))
            .filter(|&l| l <= self.logical_len)
            .ok_or_else(|| format!("offset {phys} is past the end of the file"))
    }

    /// Copy `len` logical bytes starting at `at` into `buf` (replacing it).
    /// The range is checked against the file before anything is allocated.
    fn read_into(&self, at: u64, len: u64, buf: &mut Vec<u8>) -> R<()> {
        let end = at
            .checked_add(len)
            .filter(|&e| e <= self.logical_len)
            .ok_or_else(|| "a section extends past the end of the file".to_string())?;
        buf.clear();
        buf.reserve(len as usize);
        let mut p = at;
        while p < end {
            let (page, within) = ((p / PAYLOAD as u64) as usize, (p % PAYLOAD as u64) as usize);
            let n = (PAYLOAD - within).min((end - p) as usize);
            let start = page * PAGE + within;
            buf.extend_from_slice(&self.bytes[start..start + n]);
            p += n as u64;
        }
        Ok(())
    }
}

// --------------------------------------------------------------------- XML

#[derive(Clone, Copy)]
enum FieldKind {
    Int { min: i64, max: i64 },
    Scaled { min: i64, max: i64, scale: f64, offset: f64 },
    Float { double: bool, min: Option<f64>, max: Option<f64> },
}

impl FieldKind {
    /// Value range of the field in real units, when it declares one.
    fn range(self) -> Option<(f64, f64)> {
        match self {
            FieldKind::Int { min, max } if (min, max) != (i64::MIN, i64::MAX) => {
                Some((min as f64, max as f64))
            }
            FieldKind::Scaled { min, max, scale, offset } => {
                Some((min as f64 * scale + offset, max as f64 * scale + offset))
            }
            FieldKind::Float { min: Some(a), max: Some(b), .. } => Some((a, b)),
            _ => None,
        }
    }
}

struct Field {
    name: String,
    kind: FieldKind,
}

#[derive(Default)]
struct ScanDesc {
    rotation: Option<[Option<f64>; 4]>,
    translation: Option<[Option<f64>; 3]>,
    /// colorRed/Green/Blue minimum and maximum, in that order.
    color_limits: [Option<f64>; 6],
    has_points: bool,
    file_offset: Option<u64>,
    record_count: Option<u64>,
    fields: Vec<Field>,
    custom_codecs: bool,
}

fn number(s: &str) -> R<f64> {
    if s.is_empty() {
        return Ok(0.0); // an omitted value is the default, zero
    }
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(format!("'{s}' is not a finite number")),
    }
}

fn attr_f64(tag: &xml::Tag, key: &str) -> R<Option<f64>> {
    tag.attr(key).map(|v| number(v.trim())).transpose()
}

fn attr_i64(tag: &xml::Tag, key: &str) -> R<Option<i64>> {
    tag.attr(key)
        .map(|v| {
            v.trim()
                .parse::<i64>()
                .map_err(|_| format!("attribute {key}='{v}' is not an integer"))
        })
        .transpose()
}

fn attr_u64(tag: &xml::Tag, key: &str) -> R<Option<u64>> {
    tag.attr(key)
        .map(|v| {
            v.trim()
                .parse::<u64>()
                .map_err(|_| format!("attribute {key}='{v}' is not a non-negative integer"))
        })
        .transpose()
}

fn parse_field(tag: &xml::Tag) -> R<Field> {
    let name = tag.name.to_string();
    let kind = match tag.attr("type").unwrap_or("") {
        "Integer" => FieldKind::Int {
            min: attr_i64(tag, "minimum")?.unwrap_or(i64::MIN),
            max: attr_i64(tag, "maximum")?.unwrap_or(i64::MAX),
        },
        "ScaledInteger" => {
            let min = attr_i64(tag, "minimum")?
                .ok_or_else(|| format!("ScaledInteger '{name}' has no minimum"))?;
            let max = attr_i64(tag, "maximum")?
                .ok_or_else(|| format!("ScaledInteger '{name}' has no maximum"))?;
            FieldKind::Scaled {
                min,
                max,
                scale: attr_f64(tag, "scale")?.unwrap_or(1.0),
                offset: attr_f64(tag, "offset")?.unwrap_or(0.0),
            }
        }
        "Float" => FieldKind::Float {
            double: match tag.attr("precision") {
                None | Some("double") => true,
                Some("single") => false,
                Some(p) => return Err(format!("Float '{name}' has unknown precision '{p}'")),
            },
            min: attr_f64(tag, "minimum")?,
            max: attr_f64(tag, "maximum")?,
        },
        other => {
            return Err(format!(
                "prototype field '{name}' has unsupported type '{other}' (supported: Integer, ScaledInteger, Float)"
            ))
        }
    };
    Ok(Field { name, kind })
}

fn parse_xml(xml_text: &str) -> R<Vec<ScanDesc>> {
    let mut rd = xml::Reader::new(xml_text);
    let mut path: Vec<&str> = Vec::new();
    let mut scans: Vec<ScanDesc> = Vec::new();
    let mut saw_root = false;
    while let Some(ev) = rd.next_event()? {
        match ev {
            Event::Start(tag) => {
                path.push(tag.name);
                let depth = path.len();
                if depth == 1 {
                    if tag.name != "e57Root" {
                        return Err(format!("the XML root is '{}', expected e57Root", tag.name));
                    }
                    saw_root = true;
                }
                if depth < 3 || path[1] != "data3D" {
                    continue;
                }
                if depth == 3 {
                    scans.push(ScanDesc::default());
                    continue;
                }
                let scan = scans.last_mut().ok_or("malformed data3D element")?;
                match (depth, path[3]) {
                    (4, "points") => {
                        if tag.attr("type") != Some("CompressedVector") {
                            return Err("'points' is not a CompressedVector".into());
                        }
                        scan.has_points = true;
                        scan.file_offset = attr_u64(&tag, "fileOffset")?;
                        scan.record_count = attr_u64(&tag, "recordCount")?;
                    }
                    (5, "pose") if tag.name == "rotation" => scan.rotation = Some([None; 4]),
                    (5, "pose") if tag.name == "translation" => scan.translation = Some([None; 3]),
                    (6, "points") if path[4] == "prototype" => {
                        if tag.attr("type") == Some("Structure") {
                            return Err(format!(
                                "prototype field '{}' is a Structure; nested prototypes are not supported",
                                tag.name
                            ));
                        }
                        scan.fields.push(parse_field(&tag)?);
                    }
                    (6, "points") if path[4] == "codecs" => scan.custom_codecs = true,
                    (d, "points") if d >= 7 && path[4] == "prototype" => {
                        return Err(format!(
                            "prototype field '{}' is nested, which is not supported",
                            path[5]
                        ))
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                let depth = path.len();
                if depth >= 5 && path[1] == "data3D" {
                    let scan = scans.last_mut().ok_or("malformed data3D element")?;
                    match (depth, path[3], path[4]) {
                        (6, "pose", "rotation") => {
                            let i = ["w", "x", "y", "z"].iter().position(|n| *n == path[5]);
                            if let (Some(i), Some(r)) = (i, scan.rotation.as_mut()) {
                                r[i] = Some(number(&rd.text()?)?);
                            }
                        }
                        (6, "pose", "translation") => {
                            let i = ["x", "y", "z"].iter().position(|n| *n == path[5]);
                            if let (Some(i), Some(t)) = (i, scan.translation.as_mut()) {
                                t[i] = Some(number(&rd.text()?)?);
                            }
                        }
                        (5, "colorLimits", name) => {
                            const NAMES: [&str; 6] = [
                                "colorRedMinimum",
                                "colorRedMaximum",
                                "colorGreenMinimum",
                                "colorGreenMaximum",
                                "colorBlueMinimum",
                                "colorBlueMaximum",
                            ];
                            if let Some(i) = NAMES.iter().position(|n| *n == name) {
                                scan.color_limits[i] = Some(number(&rd.text()?)?);
                            }
                        }
                        _ => {}
                    }
                }
                path.pop();
            }
        }
    }
    if !saw_root {
        return Err("the XML section has no e57Root element".into());
    }
    Ok(scans)
}

// ------------------------------------------------------------- bytestreams

#[derive(Clone, Copy)]
enum Enc {
    /// Fixed-width unsigned values stored minus `min`; zero bits means the
    /// field is the constant `min`.
    Bits { bits: u32, min: i128, scale: f64, offset: f64 },
    F32,
    F64,
}

/// Bit width needed for integers in `min..=max`.
fn bit_width(min: i64, max: i64) -> R<u32> {
    let range = i128::from(max) - i128::from(min) + 1;
    if range < 1 {
        return Err(format!("integer range {min}..={max} is empty"));
    }
    Ok(128 - ((range - 1) as u128).leading_zeros())
}

struct Column {
    enc: Enc,
    out: Vec<f64>,
    acc: u128,
    nbits: u32,
    carry: [u8; 8],
    ncarry: usize,
}

impl Column {
    fn new(kind: FieldKind) -> R<Self> {
        let enc = match kind {
            FieldKind::Int { min, max } => Enc::Bits {
                bits: bit_width(min, max)?,
                min: i128::from(min),
                scale: 1.0,
                offset: 0.0,
            },
            FieldKind::Scaled { min, max, scale, offset } => {
                if !scale.is_finite() || !offset.is_finite() {
                    return Err("a ScaledInteger has a non-finite scale or offset".into());
                }
                Enc::Bits { bits: bit_width(min, max)?, min: i128::from(min), scale, offset }
            }
            FieldKind::Float { double, .. } => {
                if double {
                    Enc::F64
                } else {
                    Enc::F32
                }
            }
        };
        Ok(Self { enc, out: Vec::new(), acc: 0, nbits: 0, carry: [0; 8], ncarry: 0 })
    }

    fn width(&self) -> u32 {
        match self.enc {
            Enc::Bits { bits, .. } => bits,
            Enc::F32 => 32,
            Enc::F64 => 64,
        }
    }

    fn constant(&self) -> Option<f64> {
        match self.enc {
            Enc::Bits { bits: 0, min, scale, offset } => Some(min as f64 * scale + offset),
            _ => None,
        }
    }

    fn complete(&self, want: usize) -> bool {
        self.constant().is_some() || self.out.len() >= want
    }

    /// Consume the next bytes of this field's stream, stopping at `want` values.
    fn feed(&mut self, bytes: &[u8], want: usize) {
        match self.enc {
            Enc::Bits { bits: 0, .. } => {}
            Enc::Bits { bits, min, scale, offset } => {
                let mask = (1u128 << bits) - 1;
                for &b in bytes {
                    if self.out.len() >= want {
                        return;
                    }
                    self.acc |= u128::from(b) << self.nbits;
                    self.nbits += 8;
                    while self.nbits >= bits && self.out.len() < want {
                        let raw = (self.acc & mask) as i128;
                        self.acc >>= bits;
                        self.nbits -= bits;
                        self.out.push((raw + min) as f64 * scale + offset);
                    }
                }
            }
            Enc::F32 | Enc::F64 => {
                let size = if matches!(self.enc, Enc::F32) { 4 } else { 8 };
                for &b in bytes {
                    if self.out.len() >= want {
                        return;
                    }
                    self.carry[self.ncarry] = b;
                    self.ncarry += 1;
                    if self.ncarry == size {
                        let c = self.carry;
                        let v = if size == 4 {
                            f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        } else {
                            f64::from_le_bytes(c)
                        };
                        self.out.push(v);
                        self.ncarry = 0;
                    }
                }
            }
        }
    }
}

/// Split one data packet into its bytestreams and feed the wanted ones.
fn feed_packet(pkt: &[u8], cols: &mut [Option<Column>], want: usize) -> R<()> {
    if u8_at(pkt, 1)? & 1 != 0 {
        return Err("compressor restarts inside a section are not supported".into());
    }
    let count = usize::from(u16_at(pkt, 4)?);
    if count != cols.len() {
        return Err(format!(
            "a data packet carries {count} bytestreams but the prototype has {} fields",
            cols.len()
        ));
    }
    let mut off = 6 + 2 * count;
    for (i, col) in cols.iter_mut().enumerate() {
        let len = usize::from(u16_at(pkt, 6 + 2 * i)?);
        let end = off
            .checked_add(len)
            .filter(|&e| e <= pkt.len())
            .ok_or("a bytestream runs past the end of its packet")?;
        if let Some(c) = col {
            c.feed(&pkt[off..end], want);
        }
        off = end;
    }
    Ok(())
}

// ------------------------------------------------------------------- scans

#[derive(Default)]
struct Decoded {
    points: Vec<DVec3>,
    colors: Option<Vec<DVec3>>,
    intensity: Option<Vec<f64>>,
}

fn pose(s: &ScanDesc) -> R<(DQuat, DVec3)> {
    let rotation = match &s.rotation {
        None => DQuat::IDENTITY,
        Some(r) => {
            let v: Vec<f64> = r
                .iter()
                .copied()
                .collect::<Option<_>>()
                .ok_or("the pose rotation needs w, x, y and z")?;
            let n = v.iter().map(|c| c * c).sum::<f64>().sqrt();
            if !n.is_finite() || (n - 1.0).abs() > 1e-3 {
                return Err(format!("the pose rotation is not a unit quaternion (length {n})"));
            }
            DQuat::from_xyzw(v[1] / n, v[2] / n, v[3] / n, v[0] / n)
        }
    };
    let translation = match &s.translation {
        None => DVec3::ZERO,
        Some(t) => {
            let v: Vec<f64> = t
                .iter()
                .copied()
                .collect::<Option<_>>()
                .ok_or("the pose translation needs x, y and z")?;
            DVec3::new(v[0], v[1], v[2])
        }
    };
    Ok((rotation, translation))
}

fn decode_scan(file: &Paged, s: &ScanDesc) -> R<Decoded> {
    if !s.has_points {
        return Err("it has no 'points' CompressedVector".into());
    }
    if s.custom_codecs {
        return Err("it declares custom codecs, which are not supported".into());
    }
    let find = |n: &str| s.fields.iter().position(|f| f.name == n);
    let (Some(ix), Some(iy), Some(iz)) = (find("cartesianX"), find("cartesianY"), find("cartesianZ"))
    else {
        return Err(
            "it has no cartesianX/Y/Z fields (only Cartesian scans are supported, not spherical-only ones)"
                .into(),
        );
    };
    let rgb = match (find("colorRed"), find("colorGreen"), find("colorBlue")) {
        (Some(r), Some(g), Some(b)) => Some([r, g, b]),
        _ => None,
    };
    let (iint, iinv) = (find("intensity"), find("cartesianInvalidState"));
    let (rotation, translation) = pose(s)?;
    let rc = s.record_count.ok_or("'points' has no recordCount")?;
    if rc == 0 {
        return Ok(Decoded::default());
    }
    let offset = s.file_offset.ok_or("'points' has no fileOffset")?;

    let mut cols: Vec<Option<Column>> = s.fields.iter().map(|_| None).collect();
    let mut wanted = vec![ix, iy, iz];
    wanted.extend(rgb.into_iter().flatten());
    wanted.extend(iint);
    wanted.extend(iinv);
    for &i in &wanted {
        cols[i] = Some(Column::new(s.fields[i].kind)?);
    }

    // Section header: id, 7 reserved bytes, logical length, data offset, index offset.
    let start = file.to_logical(offset)?;
    let mut buf = Vec::new();
    file.read_into(start, SECTION_HEADER, &mut buf)?;
    if u8_at(&buf, 0)? != 1 {
        return Err(format!("offset {offset} is not a CompressedVector section"));
    }
    let section_len = u64_at(&buf, 8)?;
    let end = start
        .checked_add(section_len)
        .filter(|&e| section_len >= SECTION_HEADER && e <= file.logical_len)
        .ok_or("the point section extends past the end of the file")?;
    let data_start = file.to_logical(u64_at(&buf, 16)?)?;
    if data_start < start + SECTION_HEADER || data_start > end {
        return Err("the data packets start outside their section".into());
    }

    // The declared count must be coverable by the bytes that are really there.
    let avail_bits = u128::from(end - data_start) * 8;
    let mut stores_bits = false;
    for c in cols.iter().flatten().filter(|c| c.width() > 0) {
        stores_bits = true;
        if u128::from(rc) * u128::from(c.width()) > avail_bits {
            return Err(format!(
                "it declares {rc} records but its section is too small to hold them: the file is truncated or damaged"
            ));
        }
    }
    if !stores_bits && rc > MAX_CONSTANT_RECORDS {
        return Err(format!("it declares {rc} records but stores no data for them"));
    }
    let want = usize::try_from(rc).map_err(|_| "the record count does not fit in memory")?;

    let mut pos = data_start;
    while pos < end && !cols.iter().flatten().all(|c| c.complete(want)) {
        file.read_into(pos, 4, &mut buf)?;
        let kind = buf[0];
        let len = usize::from(u16::from_le_bytes([buf[2], buf[3]])) + 1;
        if len % 4 != 0 {
            return Err(format!("a packet length of {len} is not a multiple of 4"));
        }
        if pos + len as u64 > end {
            return Err("a packet runs past the end of its section".into());
        }
        file.read_into(pos, len as u64, &mut buf)?;
        match kind {
            1 => feed_packet(&buf, &mut cols, want)?,
            0 | 2 => {} // index and empty packets carry no points
            k => return Err(format!("unknown packet type {k}")),
        }
        pos += len as u64;
    }
    for c in cols.iter_mut().flatten() {
        if let Some(v) = c.constant() {
            c.out = vec![v; want];
        } else if c.out.len() < want {
            return Err(format!(
                "the section holds only {} of {want} records: the file is truncated or damaged",
                c.out.len()
            ));
        }
    }
    let get = |i: usize| -> R<&[f64]> {
        cols[i]
            .as_ref()
            .map(|c| c.out.as_slice())
            .ok_or_else(|| "internal error: a wanted column is missing".to_string())
    };
    let (xs, ys, zs) = (get(ix)?, get(iy)?, get(iz)?);
    let rgb_cols = match rgb {
        Some(r) => Some([get(r[0])?, get(r[1])?, get(r[2])?]),
        None => None,
    };
    let int_col = iint.map(get).transpose()?;
    let inv_col = iinv.map(get).transpose()?;

    // Colour channels are mapped to 0..1 by the scan's colorLimits, else by
    // the field's own declared range; a channel with neither is an error.
    let colour_range = |channel: usize| -> R<(f64, f64)> {
        let (lo, hi) = match (s.color_limits[2 * channel], s.color_limits[2 * channel + 1]) {
            (Some(lo), Some(hi)) => (lo, hi),
            _ => rgb
                .and_then(|r| s.fields[r[channel]].kind.range())
                .ok_or_else(|| format!("colour channel {channel} has no declared range"))?,
        };
        if hi > lo {
            Ok((lo, hi))
        } else {
            Err(format!("colour channel {channel} has an empty range {lo}..{hi}"))
        }
    };
    let ranges = match rgb {
        Some(_) => Some([colour_range(0)?, colour_range(1)?, colour_range(2)?]),
        None => None,
    };

    let mut out = Decoded {
        points: Vec::with_capacity(want.min(1 << 20)),
        colors: rgb_cols.map(|_| Vec::new()),
        intensity: int_col.map(|_| Vec::new()),
    };
    for k in 0..want {
        if inv_col.is_some_and(|c| c[k] != 0.0) {
            continue;
        }
        let local = DVec3::new(xs[k], ys[k], zs[k]);
        out.points.push((rotation * local + translation) * M_TO_MM);
        // Float fields can hold NaN or infinity. `f64::clamp` passes NaN
        // through and would turn infinity into a plausible 1.0, so a kept
        // point with a non-finite attribute value is refused, like a
        // non-finite coordinate. Dropped points never get here.
        if let (Some(cs), Some(c), Some(r)) = (out.colors.as_mut(), rgb_cols, ranges) {
            let mut rgb_out = [0.0; 3];
            for (n, slot) in rgb_out.iter_mut().enumerate() {
                let t = (c[n][k] - r[n].0) / (r[n].1 - r[n].0);
                if !t.is_finite() {
                    return Err(format!("record {k} has a colour channel value that is not a finite number"));
                }
                *slot = t.clamp(0.0, 1.0);
            }
            cs.push(DVec3::from_array(rgb_out));
        }
        if let (Some(v), Some(c)) = (out.intensity.as_mut(), int_col) {
            if !c[k].is_finite() {
                return Err(format!("record {k} has an intensity that is not a finite number"));
            }
            v.push(c[k]);
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod testing {
    //! Builds small E57 files for tests (here and in `lib.rs`).

    use super::{crc32c, PAGE, PAYLOAD};

    /// Test-side description of one prototype field and its values.
    #[derive(Clone)]
    pub enum Enc {
        Scaled { min: i64, max: i64, scale: f64, offset: f64 },
        Int { min: i64, max: i64 },
        F32,
        F64,
    }

    #[derive(Clone)]
    pub struct Col {
        pub name: &'static str,
        pub enc: Enc,
        pub vals: Vec<f64>,
    }

    #[derive(Clone)]
    pub struct ScanSpec {
        pub cols: Vec<Col>,
        /// Unit quaternion (w, x, y, z) and translation in metres.
        pub pose: Option<([f64; 4], [f64; 3])>,
        pub color_limits: Option<(i64, i64)>,
        /// Bytes of each bytestream placed in one data packet.
        pub chunk: usize,
        /// Put an index packet and an empty packet after every data packet.
        pub filler_packets: bool,
        pub packet_flags: u8,
        pub codecs: &'static str,
        pub record_count: Option<u64>,
    }

    impl ScanSpec {
        pub fn new(cols: Vec<Col>) -> Self {
            Self {
                cols,
                pose: None,
                color_limits: None,
                chunk: 4096,
                filler_packets: false,
                packet_flags: 0,
                codecs: "<codecs type=\"Vector\" allowHeterogeneousChildren=\"1\"/>",
                record_count: None,
            }
        }
    }

    fn bits_for(min: i64, max: i64) -> u32 {
        let range = i128::from(max) - i128::from(min) + 1;
        let mut bits = 0;
        while (1i128 << bits) < range {
            bits += 1;
        }
        bits
    }

    fn pack_bits(vals: &[u64], bits: u32) -> Vec<u8> {
        let mut out = vec![0u8; (vals.len() * bits as usize).div_ceil(8)];
        for (n, v) in vals.iter().enumerate() {
            for j in 0..bits as usize {
                if (v >> j) & 1 == 1 {
                    let at = n * bits as usize + j;
                    out[at / 8] |= 1 << (at % 8);
                }
            }
        }
        out
    }

    fn stream(c: &Col) -> Vec<u8> {
        match c.enc {
            Enc::Scaled { min, max, scale, offset } => {
                let raw: Vec<u64> = c
                    .vals
                    .iter()
                    .map(|v| (((v - offset) / scale).round() as i64 - min) as u64)
                    .collect();
                pack_bits(&raw, bits_for(min, max))
            }
            Enc::Int { min, max } => {
                let raw: Vec<u64> = c.vals.iter().map(|v| (*v as i64 - min) as u64).collect();
                pack_bits(&raw, bits_for(min, max))
            }
            Enc::F32 => c.vals.iter().flat_map(|v| (*v as f32).to_le_bytes()).collect(),
            Enc::F64 => c.vals.iter().flat_map(|v| v.to_le_bytes()).collect(),
        }
    }

    fn proto(c: &Col) -> String {
        let n = c.name;
        match c.enc {
            Enc::Scaled { min, max, scale, offset } => format!(
                "<{n} type=\"ScaledInteger\" minimum=\"{min}\" maximum=\"{max}\" scale=\"{scale}\" offset=\"{offset}\"/>"
            ),
            Enc::Int { min, max } => format!("<{n} type=\"Integer\" minimum=\"{min}\" maximum=\"{max}\"/>"),
            Enc::F32 => format!("<{n} type=\"Float\" precision=\"single\"/>"),
            Enc::F64 => format!("<{n} type=\"Float\"/>"),
        }
    }

    fn pad4(v: &mut Vec<u8>) {
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
    }

    /// Logical offset to physical file offset.
    fn phys(logical: usize) -> u64 {
        ((logical / PAYLOAD) * PAGE + logical % PAYLOAD) as u64
    }

    fn packets(streams: &[Vec<u8>], chunk: usize, filler: bool, flags: u8) -> Vec<u8> {
        let mut out = Vec::new();
        let mut pos = vec![0usize; streams.len()];
        loop {
            let mut parts: Vec<&[u8]> = Vec::new();
            for (s, p) in streams.iter().zip(pos.iter_mut()) {
                let end = (*p + chunk).min(s.len());
                parts.push(&s[*p..end]);
                *p = end;
            }
            if parts.iter().all(|p| p.is_empty()) {
                break;
            }
            let mut pkt = vec![1u8, flags, 0, 0];
            pkt.extend((parts.len() as u16).to_le_bytes());
            for p in &parts {
                pkt.extend((p.len() as u16).to_le_bytes());
            }
            for p in &parts {
                pkt.extend_from_slice(p);
            }
            pad4(&mut pkt);
            let len = (pkt.len() - 1) as u16;
            pkt[2..4].copy_from_slice(&len.to_le_bytes());
            out.extend(pkt);
            if filler {
                // Index packet: 16-byte header and one 16-byte entry.
                let mut idx = vec![0u8, 0, 31, 0, 1, 0, 0];
                idx.resize(32, 0);
                out.extend(idx);
                // Empty packet.
                out.extend([2u8, 0, 7, 0, 0, 0, 0, 0]);
            }
        }
        out
    }

    fn scan_xml(s: &ScanSpec, section_phys: u64) -> String {
        let n = s.cols.first().map_or(0, |c| c.vals.len()) as u64;
        let pose = s.pose.map_or(String::new(), |(q, t)| {
            format!(
                "<pose type=\"Structure\"><rotation type=\"Structure\"><w type=\"Float\">{}</w><x type=\"Float\">{}</x><y type=\"Float\">{}</y><z type=\"Float\">{}</z></rotation><translation type=\"Structure\"><x type=\"Float\">{}</x><y type=\"Float\">{}</y><z type=\"Float\">{}</z></translation></pose>",
                q[0], q[1], q[2], q[3], t[0], t[1], t[2]
            )
        });
        let limits = s.color_limits.map_or(String::new(), |(lo, hi)| {
            let mut x = String::from("<colorLimits type=\"Structure\">");
            for c in ["Red", "Green", "Blue"] {
                x += &format!(
                    "<color{c}Minimum type=\"Integer\">{lo}</color{c}Minimum><color{c}Maximum type=\"Integer\">{hi}</color{c}Maximum>"
                );
            }
            x + "</colorLimits>"
        });
        let fields: String = s.cols.iter().map(proto).collect();
        format!(
            "<vectorChild type=\"Structure\"><guid type=\"String\"><![CDATA[{{TEST}}]]></guid>{pose}{limits}<points type=\"CompressedVector\" fileOffset=\"{section_phys}\" recordCount=\"{}\"><prototype type=\"Structure\">{fields}</prototype>{}</points></vectorChild>",
            s.record_count.unwrap_or(n),
            s.codecs
        )
    }

    /// The file's payload with the page checksums left out. Header fields are final.
    pub fn build_logical(scans: &[ScanSpec]) -> Vec<u8> {
        let mut l = vec![0u8; 48];
        let mut scan_xml_parts = Vec::new();
        for s in scans {
            pad4(&mut l);
            let start = l.len();
            let streams: Vec<Vec<u8>> = s.cols.iter().map(stream).collect();
            let body = packets(&streams, s.chunk, s.filler_packets, s.packet_flags);
            l.push(1);
            l.extend([0u8; 7]);
            l.extend(((32 + body.len()) as u64).to_le_bytes());
            l.extend(phys(start + 32).to_le_bytes());
            l.extend(0u64.to_le_bytes());
            l.extend(body);
            scan_xml_parts.push(scan_xml(s, phys(start)));
        }
        pad4(&mut l);
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<e57Root type=\"Structure\" xmlns=\"http://www.astm.org/COMMITTEE/E57/2010-e57-v1.0\"><formatName type=\"String\"><![CDATA[ASTM E57 3D Imaging Data File]]></formatName><versionMajor type=\"Integer\">1</versionMajor><versionMinor type=\"Integer\">0</versionMinor><data3D type=\"Vector\" allowHeterogeneousChildren=\"1\">{}</data3D></e57Root>\n",
            scan_xml_parts.concat()
        );
        let xml_at = l.len();
        l.extend(xml.as_bytes());
        let pages = l.len().div_ceil(PAYLOAD);
        l.resize(pages * PAYLOAD, 0);
        l[0..8].copy_from_slice(b"ASTM-E57");
        l[8..12].copy_from_slice(&1u32.to_le_bytes());
        l[12..16].copy_from_slice(&0u32.to_le_bytes());
        l[16..24].copy_from_slice(&((pages * PAGE) as u64).to_le_bytes());
        l[24..32].copy_from_slice(&phys(xml_at).to_le_bytes());
        l[32..40].copy_from_slice(&(xml.len() as u64).to_le_bytes());
        l[40..48].copy_from_slice(&(PAGE as u64).to_le_bytes());
        l
    }

    /// Add a big-endian CRC-32C after every 1020 payload bytes.
    pub fn seal(logical: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in logical.chunks(PAYLOAD) {
            let mut page = chunk.to_vec();
            page.resize(PAYLOAD, 0);
            let crc = crc32c(&page);
            out.extend(page);
            out.extend(crc.to_be_bytes());
        }
        out
    }

    pub fn build(scans: &[ScanSpec]) -> Vec<u8> {
        seal(&build_logical(scans))
    }

    /// Coordinates in metres on a 0.5 mm grid, so scaled integers are exact.
    pub fn grid(n: usize, salt: usize) -> Vec<f64> {
        (0..n)
            .map(|i| (((i * 37 + salt * 101) % 4001) as i64 - 2000) as f64 * 0.0005)
            .collect()
    }

    pub fn xyz_scaled(n: usize) -> Vec<Col> {
        ["cartesianX", "cartesianY", "cartesianZ"]
            .into_iter()
            .enumerate()
            .map(|(k, name)| Col {
                name,
                enc: Enc::Scaled { min: -4000, max: 4000, scale: 0.0005, offset: 0.0 },
                vals: grid(n, k + 1),
            })
            .collect()
    }

    /// Small valid file used by `lib.rs` tests: 40 points, one scan, colours.
    pub fn sample_file() -> Vec<u8> {
        let mut cols = xyz_scaled(40);
        for (k, name) in ["colorRed", "colorGreen", "colorBlue"].into_iter().enumerate() {
            cols.push(Col {
                name,
                enc: Enc::Int { min: 0, max: 255 },
                vals: (0..40).map(|i| ((i * (k + 3)) % 256) as f64).collect(),
            });
        }
        build(&[ScanSpec::new(cols)])
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    // The test builder's description of a field, not the reader's `Enc`.
    use super::testing::Enc;

    fn rd(bytes: &[u8]) -> IoResult<Geometry> {
        read(bytes, "t.e57")
    }

    fn err_text(bytes: &[u8]) -> String {
        rd(bytes).expect_err("expected an error").to_string()
    }

    #[test]
    fn crc32c_matches_the_catalogue_check_value() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn scaled_integer_scan_with_colour_intensity_and_pose() {
        // Enough points that the section and the XML cross several pages and
        // every bytestream is split across many packets mid-value.
        let n = 700;
        let mut cols = xyz_scaled(n);
        for (k, name) in ["colorRed", "colorGreen", "colorBlue"].into_iter().enumerate() {
            cols.push(Col {
                name,
                enc: Enc::Int { min: 0, max: 255 },
                vals: (0..n).map(|i| ((i * (k + 3)) % 256) as f64).collect(),
            });
        }
        cols.push(Col {
            name: "intensity",
            enc: Enc::F32,
            vals: (0..n).map(|i| i as f64 * 0.25).collect(),
        });
        let mut spec = ScanSpec::new(cols.clone());
        spec.chunk = 7;
        spec.filler_packets = true;
        let r = std::f64::consts::FRAC_1_SQRT_2;
        spec.pose = Some(([r, 0.0, 0.0, r], [1.0, -2.0, 0.5])); // +90 degrees about Z
        let bytes = build(&[spec]);
        assert!(bytes.len() > 3 * PAGE, "fixture should span several pages");
        let g = rd(&bytes).unwrap();
        assert_eq!(g.point_count(), n);
        let (xs, ys, zs) = (&cols[0].vals, &cols[1].vals, &cols[2].vals);
        for i in 0..n {
            // Rotation by +90 degrees about Z maps (x, y) to (-y, x).
            let want = DVec3::new(-ys[i] + 1.0, xs[i] - 2.0, zs[i] + 0.5) * 1000.0;
            assert!((g.points[i] - want).abs().max_element() < 1e-6, "point {i}");
        }
        let cd = g.point_attribs.vec3s("Cd").unwrap();
        assert!((cd[10].x - (30.0 % 256.0) / 255.0).abs() < 1e-12);
        assert!((cd[10].y - (40.0 % 256.0) / 255.0).abs() < 1e-12);
        assert!((cd[10].z - (50.0 % 256.0) / 255.0).abs() < 1e-12);
        let it = g.point_attribs.floats("intensity").unwrap();
        assert_eq!(it[7], 1.75);
    }

    #[test]
    fn float_scans_are_merged_in_order_with_their_own_poses() {
        let make = |n: usize, salt: usize, double: bool| -> ScanSpec {
            let enc = if double { Enc::F64 } else { Enc::F32 };
            ScanSpec::new(
                ["cartesianX", "cartesianY", "cartesianZ"]
                    .into_iter()
                    .enumerate()
                    .map(|(k, name)| Col { name, enc: enc.clone(), vals: grid(n, salt + k) })
                    .collect(),
            )
        };
        let mut a = make(30, 1, true);
        a.pose = Some(([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 10.0]));
        let b = make(20, 5, false);
        let g = rd(&build(&[a, b])).unwrap();
        assert_eq!(g.point_count(), 50);
        let first = DVec3::new(grid(30, 1)[0], grid(30, 2)[0], grid(30, 3)[0]);
        assert!((g.points[0] - (first + DVec3::Z * 10.0) * 1000.0).abs().max_element() < 1e-9);
        let second = DVec3::new(grid(20, 5)[0], grid(20, 6)[0], grid(20, 7)[0]);
        // The second scan is stored as f32.
        assert!((g.points[30] - second * 1000.0).abs().max_element() < 1e-3);
        assert!(!g.point_attribs.contains("Cd"));
    }

    #[test]
    fn colour_is_dropped_unless_every_scan_has_it() {
        let with_colour = |n| {
            let mut cols = xyz_scaled(n);
            for name in ["colorRed", "colorGreen", "colorBlue"] {
                cols.push(Col { name, enc: Enc::Int { min: 0, max: 255 }, vals: vec![255.0; n] });
            }
            ScanSpec::new(cols)
        };
        let g = rd(&build(&[with_colour(5), ScanSpec::new(xyz_scaled(5))])).unwrap();
        assert_eq!(g.point_count(), 10);
        assert!(!g.point_attribs.contains("Cd"));
        let g = rd(&build(&[with_colour(5), with_colour(3)])).unwrap();
        assert_eq!(g.point_attribs.vec3s("Cd").unwrap().len(), 8);
    }

    #[test]
    fn colour_limits_override_the_field_range() {
        let n = 4;
        let mut cols = xyz_scaled(n);
        for name in ["colorRed", "colorGreen", "colorBlue"] {
            cols.push(Col {
                name,
                enc: Enc::Int { min: 0, max: 65535 },
                vals: vec![0.0, 1000.0, 2000.0, 4000.0],
            });
        }
        let mut spec = ScanSpec::new(cols);
        spec.color_limits = Some((0, 4000));
        let g = rd(&build(&[spec])).unwrap();
        let cd = g.point_attribs.vec3s("Cd").unwrap();
        assert!((cd[1].x - 0.25).abs() < 1e-12 && (cd[3].z - 1.0).abs() < 1e-12);
    }

    #[test]
    fn invalid_state_points_are_dropped_and_constants_expand() {
        let n = 12;
        let mut cols = xyz_scaled(n);
        cols[2] = Col {
            name: "cartesianZ",
            enc: Enc::Scaled { min: 7, max: 7, scale: 0.001, offset: 0.5 }, // constant 0.507 m
            vals: vec![],
        };
        cols.push(Col {
            name: "cartesianInvalidState",
            enc: Enc::Int { min: 0, max: 2 },
            vals: (0..n).map(|i| if i % 4 == 3 { 2.0 } else { 0.0 }).collect(),
        });
        let mut spec = ScanSpec::new(cols);
        spec.record_count = Some(n as u64);
        let g = rd(&build(&[spec])).unwrap();
        assert_eq!(g.point_count(), 9);
        assert!(g.points.iter().all(|p| (p.z - 507.0).abs() < 1e-9));
        let xs = grid(n, 1);
        assert!((g.points[3].x - xs[4] * 1000.0).abs() < 1e-9);
    }

    /// Float scan with six points, colour as 32-bit floats limited to 0..255
    /// and a 64-bit intensity; `edit` changes the named column before the
    /// file is built.
    fn float_attribute_file(edit: impl FnOnce(&mut Vec<Col>)) -> Vec<u8> {
        let n = 6;
        let mut cols = xyz_scaled(n);
        for name in ["colorRed", "colorGreen", "colorBlue"] {
            cols.push(Col { name, enc: Enc::F32, vals: vec![0.0, 51.0, 102.0, 153.0, 204.0, 255.0] });
        }
        cols.push(Col { name: "intensity", enc: Enc::F64, vals: (0..n).map(|i| i as f64 * 0.5).collect() });
        edit(&mut cols);
        let mut spec = ScanSpec::new(cols);
        spec.color_limits = Some((0, 255));
        build(&[spec])
    }

    fn set_value(cols: &mut [Col], name: &str, at: usize, v: f64) {
        cols.iter_mut().find(|c| c.name == name).expect("column").vals[at] = v;
    }

    #[test]
    fn float_attributes_that_are_finite_read_back_finite() {
        let g = rd(&float_attribute_file(|_| {})).unwrap();
        assert_eq!(g.point_count(), 6);
        let it = g.point_attribs.floats("intensity").unwrap();
        assert!(it.iter().all(|v| v.is_finite()));
        assert_eq!(it[5], 2.5);
        let cd = g.point_attribs.vec3s("Cd").unwrap();
        assert!(cd.iter().all(|c| c.is_finite()));
        assert!((cd[3].y - 0.6).abs() < 1e-6);
    }

    #[test]
    fn non_finite_intensity_is_a_clear_error() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let bytes = float_attribute_file(|cols| set_value(cols, "intensity", 3, bad));
            let msg = err_text(&bytes);
            assert!(msg.contains("intensity") && msg.contains("not a finite number"), "{bad}: {msg}");
        }
    }

    #[test]
    fn non_finite_colour_is_a_clear_error() {
        // f64::clamp passes NaN through, and infinity would clamp to a valid
        // looking 1.0: both are damaged data and must be refused.
        for channel in ["colorRed", "colorGreen", "colorBlue"] {
            for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let bytes = float_attribute_file(|cols| set_value(cols, channel, 2, bad));
                let msg = err_text(&bytes);
                assert!(msg.contains("colour") && msg.contains("not a finite number"), "{channel} {bad}: {msg}");
            }
        }
    }

    #[test]
    fn non_finite_attributes_of_dropped_points_do_not_matter() {
        // A point flagged invalid never reaches the geometry, so what its
        // attributes hold is irrelevant; every kept value is finite.
        let bytes = float_attribute_file(|cols| {
            set_value(cols, "intensity", 1, f64::NAN);
            set_value(cols, "colorRed", 1, f64::INFINITY);
            cols.push(Col {
                name: "cartesianInvalidState",
                enc: Enc::Int { min: 0, max: 2 },
                vals: vec![0.0, 2.0, 0.0, 0.0, 0.0, 0.0],
            });
        });
        let g = rd(&bytes).unwrap();
        assert_eq!(g.point_count(), 5);
        assert!(g.point_attribs.floats("intensity").unwrap().iter().all(|v| v.is_finite()));
        assert!(g.point_attribs.vec3s("Cd").unwrap().iter().all(|c| c.is_finite()));
    }

    #[test]
    fn empty_scan_gives_an_empty_cloud() {
        let g = rd(&build(&[ScanSpec::new(xyz_scaled(0))])).unwrap();
        assert_eq!(g.point_count(), 0);
    }

    #[test]
    fn every_corrupted_byte_is_caught_by_a_checksum() {
        let good = build(&[ScanSpec::new(xyz_scaled(600))]);
        assert!(rd(&good).is_ok());
        assert!(good.len() >= 3 * PAGE);
        for i in 0..good.len() {
            let mut bad = good.clone();
            bad[i] ^= 0x01;
            let e = rd(&bad).expect_err(&format!("byte {i} flipped"));
            // The signature bytes have their own message; everything else is a checksum.
            if i >= 8 {
                assert!(e.to_string().contains("checksum"), "byte {i}: {e}");
            }
        }
    }

    #[test]
    fn truncation_at_every_length_is_an_error_not_a_panic() {
        let good = build(&[ScanSpec::new(xyz_scaled(600))]);
        for len in 0..good.len() {
            assert!(rd(&good[..len]).is_err(), "length {len}");
        }
        // Cut exactly on a page boundary: the checksums pass, the header does not.
        let e = err_text(&good[..PAGE * 2]);
        assert!(e.contains("truncated"), "{e}");
        // A whole extra page with a valid checksum is rejected too: the header
        // length no longer matches the file.
        let mut longer = good.clone();
        let mut extra = vec![0u8; PAYLOAD];
        extra.extend(crc32c(&extra).to_be_bytes());
        longer.extend(extra);
        assert!(err_text(&longer).contains("declares"));
        // Zero bytes without a checksum are caught by the checksum instead.
        let mut junk = good.clone();
        junk.extend(vec![0u8; PAGE]);
        assert!(err_text(&junk).contains("checksum"));
    }

    #[test]
    fn structural_damage_behind_valid_checksums_is_an_error_not_a_panic() {
        let mut spec = ScanSpec::new(xyz_scaled(40));
        spec.chunk = 11;
        spec.filler_packets = true;
        let logical = build_logical(&[spec]);
        assert!(rd(&seal(&logical)).is_ok());
        for i in 0..logical.len() {
            for mask in [0xFFu8, 0x80, 0x01] {
                let mut bad = logical.clone();
                bad[i] ^= mask;
                let _ = rd(&seal(&bad)); // any outcome but a panic
            }
        }
        // Zeroing the tail where the sections live must not panic either.
        for from in (48..logical.len()).step_by(13) {
            let mut bad = logical.clone();
            bad[from..].fill(0);
            let _ = rd(&seal(&bad));
        }
    }

    #[test]
    fn declared_counts_are_checked_against_the_data() {
        let mut spec = ScanSpec::new(xyz_scaled(10));
        spec.record_count = Some(u64::MAX);
        let e = err_text(&build(&[spec.clone()]));
        assert!(e.contains("too small") || e.contains("truncated"), "{e}");
        spec.record_count = Some(11);
        assert!(err_text(&build(&[spec])).contains("holds only"));
        // A huge count over constant-only fields must not allocate.
        let consts = ["cartesianX", "cartesianY", "cartesianZ"]
            .into_iter()
            .map(|name| Col {
                name,
                enc: Enc::Scaled { min: 0, max: 0, scale: 1.0, offset: 0.0 },
                vals: vec![],
            })
            .collect();
        let mut spec = ScanSpec::new(consts);
        spec.record_count = Some(1 << 40);
        assert!(err_text(&build(&[spec])).contains("stores no data"));
    }

    #[test]
    fn unsupported_encodings_are_explicit_errors() {
        // Spherical-only scan.
        let sph = ["sphericalRange", "sphericalAzimuth", "sphericalElevation"]
            .into_iter()
            .map(|name| Col { name, enc: Enc::F64, vals: vec![1.0, 2.0] })
            .collect();
        assert!(err_text(&build(&[ScanSpec::new(sph)])).contains("cartesian"));
        // Custom codecs.
        let mut spec = ScanSpec::new(xyz_scaled(3));
        spec.codecs = "<codecs type=\"Vector\" allowHeterogeneousChildren=\"1\"><vectorChild type=\"Structure\"/></codecs>";
        assert!(err_text(&build(&[spec])).contains("codecs"));
        // Compressor restart flag.
        let mut spec = ScanSpec::new(xyz_scaled(3));
        spec.packet_flags = 1;
        assert!(err_text(&build(&[spec])).contains("restart"));
    }

    #[test]
    fn unknown_prototype_type_and_missing_pose_parts_are_errors() {
        let xml = |proto: &str, pose: &str| {
            format!(
                "<e57Root type=\"Structure\"><data3D type=\"Vector\"><vectorChild type=\"Structure\">{pose}<points type=\"CompressedVector\" fileOffset=\"0\" recordCount=\"1\"><prototype type=\"Structure\">{proto}</prototype></points></vectorChild></data3D></e57Root>"
            )
        };
        let bad_type = parse_xml(&xml("<cartesianX type=\"String\"/>", ""));
        assert!(bad_type.err().unwrap().contains("unsupported type"));
        let nested = parse_xml(&xml("<a type=\"Structure\"><b type=\"Float\"/></a>", ""));
        assert!(nested.err().unwrap().contains("nested"));
        let ok = parse_xml(&xml("<cartesianX type=\"Float\"/>", "")).unwrap();
        assert_eq!(ok[0].fields.len(), 1);
        let partial = "<pose type=\"Structure\"><rotation type=\"Structure\"><w type=\"Float\">1</w></rotation></pose>";
        let scans = parse_xml(&xml("<cartesianX type=\"Float\"/>", partial)).unwrap();
        assert!(pose(&scans[0]).err().unwrap().contains("w, x, y and z"));
        assert!(parse_xml("<notE57/>").err().unwrap().contains("e57Root"));
        assert!(parse_xml("<e57Root><data3D><vectorChild><pose><rotation><w>1<!--c--></w></rotation></pose></vectorChild></data3D></e57Root>")
            .err()
            .unwrap()
            .contains("not a finite number"));
    }

    #[test]
    fn non_unit_quaternion_is_rejected() {
        let mut spec = ScanSpec::new(xyz_scaled(3));
        spec.pose = Some(([2.0, 0.0, 0.0, 0.0], [0.0; 3]));
        assert!(err_text(&build(&[spec])).contains("unit quaternion"));
    }

    #[test]
    fn missing_signature_and_short_files() {
        assert!(err_text(b"").contains("not an E57"));
        assert!(err_text(b"ASTM-E5").contains("not an E57"));
        assert!(err_text(&vec![0u8; 2048]).contains("not an E57"));
        let mut logical = build_logical(&[ScanSpec::new(xyz_scaled(3))]);
        logical[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert!(err_text(&seal(&logical)).contains("version 2.0"));
    }
}
