//! The block-linear layout of the Switch's GPU (Tegra X1) and the Switch 2's (Orin), in which textures are stored.
//!
//! The surface is cut into GOBs (64 bytes x 8 rows, 512 bytes), stacked `block_height` GOBs high into blocks; blocks
//! are laid out left to right, then top to bottom. Inside a GOB, bytes follow a fixed pattern of 16-byte runs,
//! which differs between the two GPUs ([`Gob`]). Here a "texel" is a compressed block (4x4 for BC, the ASTC block
//! size) or a pixel for uncompressed formats.
//!
//! The GOB patterns are those of the Tegra X1 and Orin technical reference manuals, as also implemented in
//! tegra_swizzle (MIT; the Orin pattern in KillzXGaming's fork). The mip-level rule (block height halves while the
//! level fits in half of it) is the one Ryujinx and tegra_swizzle use.

const GOB_WIDTH: usize = 64;
const GOB_HEIGHT: usize = 8;
const GOB_SIZE: usize = GOB_WIDTH * GOB_HEIGHT;

/// The byte order inside a GOB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gob {
    /// Switch (Tegra X1, Maxwell): 2 x 4 pairs of 16-byte rows.
    TegraX1,
    /// Switch 2 (Orin, Ampere): 2 x 2 groups of four 16-byte rows.
    Orin,
}

impl Gob {
    /// Offset of the byte at (`x`, `y`) inside a GOB (`x` < 64, `y` < 8).
    fn offset(self, x: usize, y: usize) -> usize {
        match self {
            Gob::TegraX1 => {
                (x / 32) * 256 + (y / 2) * 64 + ((x % 32) / 16) * 32 + (y % 2) * 16 + x % 16
            }
            Gob::Orin => {
                (x / 32) * 256 + (y / 4) * 128 + ((x % 32) / 16) * 64 + (y % 4) * 16 + x % 16
            }
        }
    }
}

/// One 2D surface (a mip level of one array layer), measured in texels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surface {
    pub width: usize,
    pub height: usize,
    pub bytes_per_texel: usize,
    /// GOBs per block, vertically (a power of two, 1 to 32).
    pub block_height: usize,
    pub gob: Gob,
}

impl Surface {
    /// Mip `level` of a texture `width` x `height` pixels, with blocks of `block_w` x `block_h` pixels and a base
    /// block height of `1 << block_height_log2` GOBs.
    pub fn mip(
        gob: Gob,
        width: u32,
        height: u32,
        level: u32,
        (block_w, block_h): (u32, u32),
        bytes_per_texel: u32,
        block_height_log2: u32,
    ) -> Self {
        let w = (width >> level).max(1).div_ceil(block_w) as usize;
        let h = (height >> level).max(1).div_ceil(block_h) as usize;
        let mut block_height = 1usize << block_height_log2.min(5);
        while block_height > 1 && h <= (block_height / 2) * GOB_HEIGHT {
            block_height /= 2;
        }
        Surface {
            width: w,
            height: h,
            bytes_per_texel: bytes_per_texel as usize,
            block_height,
            gob,
        }
    }

    fn width_in_gobs(&self) -> usize {
        (self.width * self.bytes_per_texel).div_ceil(GOB_WIDTH)
    }

    /// Bytes the surface takes in block-linear layout (whole blocks).
    pub fn swizzled_size(&self) -> usize {
        let block_rows = self.height.div_ceil(GOB_HEIGHT * self.block_height);
        block_rows * self.width_in_gobs() * self.block_height * GOB_SIZE
    }

    /// Bytes the surface takes as rows of texels.
    pub fn linear_size(&self) -> usize {
        self.width * self.height * self.bytes_per_texel
    }

    /// Where the byte at (`x` bytes, `y` rows) of the surface is in block-linear layout.
    fn address(&self, x: usize, y: usize, width_in_gobs: usize) -> usize {
        let block_size = GOB_SIZE * self.block_height;
        let rows_per_block = GOB_HEIGHT * self.block_height;
        let block = (y / rows_per_block) * width_in_gobs + x / GOB_WIDTH;
        let gob = (y % rows_per_block) / GOB_HEIGHT;
        block * block_size + gob * GOB_SIZE + self.gob.offset(x % GOB_WIDTH, y % GOB_HEIGHT)
    }

    /// Block-linear `src` (at least `swizzled_size` bytes) to rows of texels.
    pub fn deswizzle(&self, src: &[u8]) -> Option<Vec<u8>> {
        if src.len() < self.swizzled_size() {
            return None;
        }
        let mut out = vec![0u8; self.linear_size()];
        self.walk(|linear, swizzled, len| {
            out[linear..linear + len].copy_from_slice(&src[swizzled..swizzled + len]);
        });
        Some(out)
    }

    /// Rows of texels (`linear_size` bytes) to block-linear layout (`swizzled_size` bytes, padding zeroed).
    pub fn swizzle(&self, src: &[u8]) -> Option<Vec<u8>> {
        if src.len() < self.linear_size() {
            return None;
        }
        let mut out = vec![0u8; self.swizzled_size()];
        self.walk(|linear, swizzled, len| {
            out[swizzled..swizzled + len].copy_from_slice(&src[linear..linear + len]);
        });
        Some(out)
    }

    /// Calls `f(linear offset, swizzled offset, length)` for every run of bytes that is contiguous in both layouts.
    fn walk(&self, mut f: impl FnMut(usize, usize, usize)) {
        // 16 bytes are always contiguous in a GOB; a texel is 1 to 16 bytes and divides 16.
        let row_bytes = self.width * self.bytes_per_texel;
        let width_in_gobs = self.width_in_gobs();
        for y in 0..self.height {
            let mut x = 0;
            while x < row_bytes {
                let len = (16 - x % 16).min(row_bytes - x);
                f(y * row_bytes + x, self.address(x, y, width_in_gobs), len);
                x += len;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_block_heights_and_sizes() {
        // An 85x85 ASTC 6x6 texture with a base block height of 2 GOBs, as found in the game: the level sizes
        // match the mip offsets in its BRTI (0x1000, 0x400, then 0x200 each).
        let sizes: Vec<usize> = (0..7)
            .map(|l| Surface::mip(Gob::Orin, 85, 85, l, (6, 6), 16, 1).swizzled_size())
            .collect();
        assert_eq!(sizes, [0x1000, 0x400, 0x200, 0x200, 0x200, 0x200, 0x200]);
        let s = Surface::mip(Gob::TegraX1, 1024, 1024, 0, (4, 4), 16, 4);
        assert_eq!((s.width, s.height, s.block_height), (256, 256, 16));
        assert_eq!(
            Surface::mip(Gob::TegraX1, 1024, 1024, 4, (4, 4), 16, 4).block_height,
            2
        );
    }

    fn one_gob(gob: Gob) -> Surface {
        Surface {
            width: 64,
            height: 8,
            bytes_per_texel: 1,
            block_height: 1,
            gob,
        }
    }

    #[test]
    fn gob_patterns() {
        // Where the 16-byte runs of each row (x = 0, 16, 32, 48) start.
        let starts = |gob: Gob| -> Vec<[usize; 4]> {
            let s = one_gob(gob);
            (0..8)
                .map(|y| [0, 16, 32, 48].map(|x| s.address(x, y, 1)))
                .collect()
        };
        let x1 = starts(Gob::TegraX1);
        assert_eq!(x1[0], [0, 32, 256, 288]);
        assert_eq!(x1[1], [16, 48, 272, 304]);
        assert_eq!(x1[2], [64, 96, 320, 352]);
        assert_eq!(x1[7], [208, 240, 464, 496]);
        let orin = starts(Gob::Orin);
        assert_eq!(orin[0], [0, 64, 256, 320]);
        assert_eq!(orin[1], [16, 80, 272, 336]);
        assert_eq!(orin[3], [48, 112, 304, 368]);
        assert_eq!(orin[4], [128, 192, 384, 448]);
        assert_eq!(orin[7], [176, 240, 432, 496]);
        for gob in [Gob::TegraX1, Gob::Orin] {
            assert_eq!(one_gob(gob).address(63, 7, 1), 511);
        }
    }

    #[test]
    fn swizzle_round_trips_and_covers_every_byte() {
        for gob in [Gob::TegraX1, Gob::Orin] {
            for (w, h, bpt, bh) in [
                (13, 9, 16, 2),
                (100, 70, 8, 4),
                (3, 200, 4, 16),
                (1, 1, 16, 1),
                (70, 5, 1, 1),
            ] {
                let s = Surface {
                    width: w,
                    height: h,
                    bytes_per_texel: bpt,
                    block_height: bh,
                    gob,
                };
                let linear: Vec<u8> = (0..s.linear_size())
                    .map(|i| (i * 7 + i / 251) as u8)
                    .collect();
                let swizzled = s.swizzle(&linear).unwrap();
                assert_eq!(swizzled.len(), s.swizzled_size());
                assert_eq!(s.deswizzle(&swizzled).unwrap(), linear);
                // No two texel bytes land on the same address.
                let mut seen = vec![false; s.swizzled_size()];
                s.walk(|_, at, len| {
                    for b in &mut seen[at..at + len] {
                        assert!(!*b);
                        *b = true;
                    }
                });
            }
        }
    }
}
