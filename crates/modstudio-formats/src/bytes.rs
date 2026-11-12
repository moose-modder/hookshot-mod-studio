//! Bounds-checked little-endian reads from a byte slice. Every format here is little-endian (Switch).

use crate::error::{Error, Result};

pub(crate) trait ReadLe {
    fn slice_at(&self, offset: usize, len: usize) -> Result<&[u8]>;

    fn array_at<const N: usize>(&self, offset: usize) -> Result<[u8; N]> {
        let s = self.slice_at(offset, N)?;
        Ok(s.try_into().expect("slice_at returned N bytes"))
    }
    fn u8_at(&self, offset: usize) -> Result<u8> {
        Ok(self.array_at::<1>(offset)?[0])
    }
    fn u16_at(&self, offset: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array_at(offset)?))
    }
    fn i16_at(&self, offset: usize) -> Result<i16> {
        Ok(i16::from_le_bytes(self.array_at(offset)?))
    }
    fn u32_at(&self, offset: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array_at(offset)?))
    }
    fn i32_at(&self, offset: usize) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array_at(offset)?))
    }
    fn u64_at(&self, offset: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array_at(offset)?))
    }
    fn f32_at(&self, offset: usize) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array_at(offset)?))
    }
    /// A 64-bit offset field, as a position in the data (0 stays 0: "none").
    fn offset_at(&self, offset: usize) -> Result<usize> {
        let v = self.u64_at(offset)?;
        usize::try_from(v)
            .map_err(|_| crate::error::invalid(format!("offset 0x{v:x} at 0x{offset:x}")))
    }
    /// A zero-terminated string starting at `offset`.
    fn cstr_at(&self, offset: usize) -> Result<&str>;
}

impl ReadLe for [u8] {
    fn slice_at(&self, offset: usize, len: usize) -> Result<&[u8]> {
        offset
            .checked_add(len)
            .and_then(|end| self.get(offset..end))
            .ok_or(Error::Truncated {
                offset,
                len,
                size: self.len(),
            })
    }

    fn cstr_at(&self, offset: usize) -> Result<&str> {
        let rest = self.get(offset..).ok_or(Error::Truncated {
            offset,
            len: 1,
            size: self.len(),
        })?;
        let end = rest.iter().position(|&b| b == 0).ok_or(Error::Truncated {
            offset,
            len: rest.len() + 1,
            size: self.len(),
        })?;
        std::str::from_utf8(&rest[..end])
            .map_err(|_| crate::error::invalid(format!("string at 0x{offset:x} is not UTF-8")))
    }
}

pub(crate) fn check_magic(data: &[u8], offset: usize, magic: &'static str) -> Result<()> {
    let found = data.slice_at(offset, magic.len())?;
    if found != magic.as_bytes() {
        return Err(Error::BadMagic {
            expected: magic,
            found: found.to_vec(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_little_endian_and_bounded() {
        let d: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, b'h', b'i', 0];
        assert_eq!(d.u16_at(0).unwrap(), 0x0201);
        assert_eq!(d.u32_at(1).unwrap(), 0x05040302);
        assert_eq!(d.u64_at(0).unwrap(), 0x0807060504030201);
        assert_eq!(d.cstr_at(8).unwrap(), "hi");
        assert!(matches!(
            d.u32_at(8),
            Err(Error::Truncated {
                offset: 8,
                len: 4,
                size: 11
            })
        ));
        assert!(d.u32_at(usize::MAX).is_err());
        assert!(d[..10].cstr_at(8).is_err());
    }
}
