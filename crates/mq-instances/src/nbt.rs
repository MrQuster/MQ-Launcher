//! Bounded, memory-safe binary NBT (Named Binary Tag) reader and writer for `level.dat` and `servers.dat`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;
use mq_core::{MqError, MqResult};

/// Maximum recursion depth allowed when parsing nested `TAG_Compound` / `TAG_List` structures.
pub(crate) const MAX_NBT_DEPTH: usize = 64;

pub(crate) const TAG_END: u8 = 0;
pub(crate) const TAG_BYTE: u8 = 1;
pub(crate) const TAG_SHORT: u8 = 2;
pub(crate) const TAG_INT: u8 = 3;
pub(crate) const TAG_LONG: u8 = 4;
pub(crate) const TAG_FLOAT: u8 = 5;
pub(crate) const TAG_DOUBLE: u8 = 6;
pub(crate) const TAG_BYTE_ARRAY: u8 = 7;
pub(crate) const TAG_STRING: u8 = 8;
pub(crate) const TAG_LIST: u8 = 9;
pub(crate) const TAG_COMPOUND: u8 = 10;
pub(crate) const TAG_INT_ARRAY: u8 = 11;
pub(crate) const TAG_LONG_ARRAY: u8 = 12;

/// Parsed binary NBT value (`tags 1..=12`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NbtValue {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<NbtValue>),
    Compound(Vec<(String, NbtValue)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtValue {
    #[inline]
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Byte(v) => Some(i32::from(*v)),
            Self::Short(v) => Some(i32::from(*v)),
            Self::Int(v) => Some(*v),
            Self::Long(v) => i32::try_from(*v).ok(),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Byte(v) => Some(i64::from(*v)),
            Self::Short(v) => Some(i64::from(*v)),
            Self::Int(v) => Some(i64::from(*v)),
            Self::Long(v) => Some(*v),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn as_u8(&self) -> Option<u8> {
        match self {
            Self::Byte(v) => Some(*v as u8),
            Self::Short(v) => u8::try_from(*v).ok(),
            Self::Int(v) => u8::try_from(*v).ok(),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Byte(v) => Some(*v != 0),
            Self::Short(v) => Some(*v != 0),
            Self::Int(v) => Some(*v != 0),
            Self::Long(v) => Some(*v != 0),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn as_list(&self) -> Option<&[NbtValue]> {
        match self {
            Self::List(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn as_int_array(&self) -> Option<&[i32]> {
        match self {
            Self::IntArray(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&NbtValue> {
        match self {
            Self::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Reads a potentially Gzip-compressed or uncompressed NBT file with a strict upper bound
/// of `max_bytes` on the decompressed byte stream so decompression bombs can never OOM.
pub(crate) fn read_nbt_file_bounded(path: &Path, max_bytes: usize) -> MqResult<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 2];
    let n = file.read(&mut magic)?;
    if n == 0 {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(0))?;

    let limit = (max_bytes as u64).saturating_add(1);
    if n == 2 && magic == [0x1F, 0x8B] {
        let mut decoder = GzDecoder::new(file).take(limit);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|err| MqError::Metadata(format!("failed decompressing Gzip NBT: {err}")))?;
        if out.len() > max_bytes {
            return Err(MqError::Archive(format!(
                "decompressed NBT exceeds maximum size of {max_bytes} bytes"
            )));
        }
        Ok(out)
    } else {
        let mut reader = file.take(limit);
        let mut out = Vec::new();
        reader.read_to_end(&mut out)?;
        if out.len() > max_bytes {
            return Err(MqError::Archive(format!(
                "raw NBT file exceeds maximum size of {max_bytes} bytes"
            )));
        }
        Ok(out)
    }
}

/// Parses a binary Java Edition NBT byte slice starting with a root `TAG_Compound` (`0x0A`).
pub(crate) fn parse_nbt_root(bytes: &[u8]) -> MqResult<(String, NbtValue)> {
    let mut reader = NbtCursor::new(bytes);
    let root_tag = reader.read_u8()?;
    if root_tag != TAG_COMPOUND {
        return Err(MqError::Metadata(format!(
            "expected root TAG_Compound (10), found tag {root_tag}"
        )));
    }
    let root_name = reader.read_string()?;
    let root_val = reader.read_compound_payload(0)?;
    Ok((root_name, root_val))
}

/// Writes a length-prefixed NBT string (`u16` big-endian length + UTF-8 bytes), returning
/// `MqError::Metadata` instead of truncating if `s.len() > u16::MAX`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_nbt_string_checked(
    buf: &mut Vec<u8>,
    field_label: &str,
    s: &str,
) -> MqResult<()> {
    let bytes = s.as_bytes();
    let len = u16::try_from(bytes.len()).map_err(|_| {
        MqError::Metadata(format!(
            "NBT string '{field_label}' length {} exceeds u16::MAX (65535)",
            bytes.len()
        ))
    })?;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(bytes);
    Ok(())
}

struct NbtCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> NbtCursor<'a> {
    #[inline]
    const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    const fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_exact_slice(&mut self, len: usize) -> MqResult<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| {
                MqError::Metadata(format!(
                    "unexpected EOF in NBT at offset {} (needed {len} bytes, remaining {})",
                    self.pos,
                    self.remaining()
                ))
            })?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    #[inline]
    fn read_u8(&mut self) -> MqResult<u8> {
        Ok(self.read_exact_slice(1)?[0])
    }

    #[inline]
    fn read_i8(&mut self) -> MqResult<i8> {
        Ok(self.read_u8()? as i8)
    }

    #[inline]
    fn read_u16(&mut self) -> MqResult<u16> {
        let b = self.read_exact_slice(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    #[inline]
    fn read_i16(&mut self) -> MqResult<i16> {
        let b = self.read_exact_slice(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    #[inline]
    fn read_i32(&mut self) -> MqResult<i32> {
        let b = self.read_exact_slice(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    #[inline]
    fn read_i64(&mut self) -> MqResult<i64> {
        let b = self.read_exact_slice(8)?;
        Ok(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_string(&mut self) -> MqResult<String> {
        let len = self.read_u16()? as usize;
        let raw = self.read_exact_slice(len)?;
        Ok(decode_modified_utf8(raw))
    }

    fn read_payload(&mut self, tag_type: u8, depth: usize) -> MqResult<NbtValue> {
        if depth > MAX_NBT_DEPTH {
            return Err(MqError::Metadata(format!(
                "NBT nesting depth exceeds maximum of {MAX_NBT_DEPTH}"
            )));
        }

        match tag_type {
            TAG_BYTE => Ok(NbtValue::Byte(self.read_i8()?)),
            TAG_SHORT => Ok(NbtValue::Short(self.read_i16()?)),
            TAG_INT => Ok(NbtValue::Int(self.read_i32()?)),
            TAG_LONG => Ok(NbtValue::Long(self.read_i64()?)),
            TAG_FLOAT => {
                let bits = self.read_i32()? as u32;
                Ok(NbtValue::Float(f32::from_bits(bits)))
            }
            TAG_DOUBLE => {
                let bits = self.read_i64()? as u64;
                Ok(NbtValue::Double(f64::from_bits(bits)))
            }
            TAG_BYTE_ARRAY => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(MqError::Metadata(format!(
                        "negative NBT TAG_Byte_Array length: {len}"
                    )));
                }
                let slice = self.read_exact_slice(len as usize)?;
                Ok(NbtValue::ByteArray(slice.to_vec()))
            }
            TAG_STRING => Ok(NbtValue::String(self.read_string()?)),
            TAG_LIST => self.read_list_payload(depth + 1),
            TAG_COMPOUND => self.read_compound_payload(depth + 1),
            TAG_INT_ARRAY => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(MqError::Metadata(format!(
                        "negative NBT TAG_Int_Array length: {len}"
                    )));
                }
                let count = len as usize;
                let byte_len = count.checked_mul(4).ok_or_else(|| {
                    MqError::Metadata("NBT TAG_Int_Array byte length overflow".to_owned())
                })?;
                let raw = self.read_exact_slice(byte_len)?;
                let mut items = Vec::with_capacity(count);
                for &chunk in raw.as_chunks::<4>().0 {
                    items.push(i32::from_be_bytes(chunk));
                }
                Ok(NbtValue::IntArray(items))
            }
            TAG_LONG_ARRAY => {
                let len = self.read_i32()?;
                if len < 0 {
                    return Err(MqError::Metadata(format!(
                        "negative NBT TAG_Long_Array length: {len}"
                    )));
                }
                let count = len as usize;
                let byte_len = count.checked_mul(8).ok_or_else(|| {
                    MqError::Metadata("NBT TAG_Long_Array byte length overflow".to_owned())
                })?;
                let raw = self.read_exact_slice(byte_len)?;
                let mut items = Vec::with_capacity(count);
                for &chunk in raw.as_chunks::<8>().0 {
                    items.push(i64::from_be_bytes(chunk));
                }
                Ok(NbtValue::LongArray(items))
            }
            other => Err(MqError::Metadata(format!(
                "unknown or invalid NBT tag ID: {other}"
            ))),
        }
    }

    fn read_list_payload(&mut self, depth: usize) -> MqResult<NbtValue> {
        if depth > MAX_NBT_DEPTH {
            return Err(MqError::Metadata(format!(
                "NBT nesting depth exceeds maximum of {MAX_NBT_DEPTH}"
            )));
        }
        let elem_tag = self.read_u8()?;
        let len = self.read_i32()?;
        if len < 0 {
            return Err(MqError::Metadata(format!(
                "negative NBT TAG_List length: {len}"
            )));
        }
        if len == 0 {
            return Ok(NbtValue::List(Vec::new()));
        }
        if !(TAG_BYTE..=TAG_LONG_ARRAY).contains(&elem_tag) {
            return Err(MqError::Metadata(format!(
                "invalid NBT TAG_List element tag ID {elem_tag} for non-empty list of length {len}"
            )));
        }
        let count = len as usize;
        // Every non-end NBT payload occupies at least 1 byte in the stream.
        if count > self.remaining() {
            return Err(MqError::Metadata(format!(
                "truncated NBT TAG_List: declared {count} items, only {} bytes remain",
                self.remaining()
            )));
        }
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(self.read_payload(elem_tag, depth)?);
        }
        Ok(NbtValue::List(items))
    }

    fn read_compound_payload(&mut self, depth: usize) -> MqResult<NbtValue> {
        if depth > MAX_NBT_DEPTH {
            return Err(MqError::Metadata(format!(
                "NBT nesting depth exceeds maximum of {MAX_NBT_DEPTH}"
            )));
        }
        let mut entries = Vec::new();
        loop {
            let tag_type = self.read_u8()?;
            if tag_type == TAG_END {
                break;
            }
            let name = self.read_string()?;
            let val = self.read_payload(tag_type, depth)?;
            entries.push((name, val));
        }
        Ok(NbtValue::Compound(entries))
    }
}

/// Decodes Java Modified UTF-8 (`0xC0 0x80` -> `\0`) or standard UTF-8 cleanly.
fn decode_modified_utf8(raw: &[u8]) -> String {
    if !raw.windows(2).any(|w| w == [0xC0, 0x80]) {
        if let Ok(s) = std::str::from_utf8(raw) {
            return s.to_owned();
        }
    }
    let mut normalized = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if i + 1 < raw.len() && raw[i] == 0xC0 && raw[i + 1] == 0x80 {
            normalized.push(0x00);
            i += 2;
        } else {
            normalized.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&normalized).into_owned()
}
