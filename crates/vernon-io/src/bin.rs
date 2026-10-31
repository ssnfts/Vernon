//! Bounds-checked little-endian field reads for the binary readers (E57, LAS).
//!
//! Every function returns an error instead of panicking when the field lies
//! outside the buffer, so a truncated or crafted file cannot index out of range.

type R<T> = Result<T, String>;

fn field<const N: usize>(b: &[u8], at: usize) -> R<[u8; N]> {
    at.checked_add(N)
        .and_then(|end| b.get(at..end))
        .and_then(|s| <[u8; N]>::try_from(s).ok())
        .ok_or_else(|| format!("data ends early: {N} bytes expected at offset {at}"))
}

pub(crate) fn u8_at(b: &[u8], at: usize) -> R<u8> {
    Ok(field::<1>(b, at)?[0])
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> R<u16> {
    Ok(u16::from_le_bytes(field(b, at)?))
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> R<u32> {
    Ok(u32::from_le_bytes(field(b, at)?))
}

pub(crate) fn i32_at(b: &[u8], at: usize) -> R<i32> {
    Ok(i32::from_le_bytes(field(b, at)?))
}

pub(crate) fn u64_at(b: &[u8], at: usize) -> R<u64> {
    Ok(u64::from_le_bytes(field(b, at)?))
}

pub(crate) fn f64_at(b: &[u8], at: usize) -> R<f64> {
    Ok(f64::from_le_bytes(field(b, at)?))
}
