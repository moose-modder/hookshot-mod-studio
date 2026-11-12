//! BC4 and BC5 (one and two channels of 8-byte blocks), unsigned and signed.
//!
//! Signed values are mapped to 0..=255 as `(v / 127) * 0.5 + 0.5`, the usual way normal maps are stored unsigned.

/// The 8 values of a BC4 block's palette, as 0..=255.
fn palette(block: &[u8], signed: bool) -> [u8; 8] {
    if signed {
        let e0 = (block[0] as i8).max(-127) as i32;
        let e1 = (block[1] as i8).max(-127) as i32;
        let mut p = [0i32; 8];
        p[0] = e0;
        p[1] = e1;
        if e0 > e1 {
            for i in 1..7 {
                p[i + 1] = round_div((7 - i as i32) * e0 + i as i32 * e1, 7);
            }
        } else {
            for i in 1..5 {
                p[i + 1] = round_div((5 - i as i32) * e0 + i as i32 * e1, 5);
            }
            p[6] = -127;
            p[7] = 127;
        }
        p.map(|v| ((v + 127) * 255 + 127).div_euclid(254) as u8)
    } else {
        let e0 = i32::from(block[0]);
        let e1 = i32::from(block[1]);
        let mut p = [0i32; 8];
        p[0] = e0;
        p[1] = e1;
        if e0 > e1 {
            for i in 1..7 {
                p[i + 1] = round_div((7 - i as i32) * e0 + i as i32 * e1, 7);
            }
        } else {
            for i in 1..5 {
                p[i + 1] = round_div((5 - i as i32) * e0 + i as i32 * e1, 5);
            }
            p[6] = 0;
            p[7] = 255;
        }
        p.map(|v| v as u8)
    }
}

fn round_div(n: i32, d: i32) -> i32 {
    (2 * n + d * n.signum()) / (2 * d)
}

/// The 16 values (row by row) of one BC4 block.
pub fn decode_bc4_block(block: &[u8], signed: bool) -> [u8; 16] {
    let p = palette(block, signed);
    let mut bits = u64::from_le_bytes([
        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
    ]);
    let mut out = [0u8; 16];
    for v in &mut out {
        *v = p[(bits & 7) as usize];
        bits >>= 3;
    }
    out
}

/// Decodes `width` x `height` pixels of BC4 (`channels` = 1) or BC5 (`channels` = 2) blocks, stored row by row,
/// into RGBA: red (and green) from the blocks, the rest 0, alpha 255.
pub fn decode(
    data: &[u8],
    width: usize,
    height: usize,
    channels: usize,
    signed: bool,
) -> Option<Vec<u8>> {
    let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
    let block_size = 8 * channels;
    if data.len() < bw * bh * block_size {
        return None;
    }
    let mut out = vec![0u8; width * height * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let block = &data[(by * bw + bx) * block_size..][..block_size];
            let r = decode_bc4_block(&block[..8], signed);
            let g = (channels == 2).then(|| decode_bc4_block(&block[8..16], signed));
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx * 4 + px, by * 4 + py);
                    if x >= width || y >= height {
                        continue;
                    }
                    let o = (y * width + x) * 4;
                    out[o] = r[py * 4 + px];
                    out[o + 1] = g.map_or(0, |g| g[py * 4 + px]);
                    out[o + 3] = 255;
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_palettes() {
        // e0 > e1: 6 interpolated values; e0 <= e1: 4 interpolated, then 0 and 255.
        assert_eq!(
            palette(&[255, 0, 0, 0, 0, 0, 0, 0], false),
            [255, 0, 219, 182, 146, 109, 73, 36]
        );
        assert_eq!(
            palette(&[0, 255, 0, 0, 0, 0, 0, 0], false),
            [0, 255, 51, 102, 153, 204, 0, 255]
        );
    }

    #[test]
    fn signed_palettes() {
        // -127 -> 0, 0 -> 128 (127.5 rounded up), 127 -> 255; -128 is read as -127.
        let p = palette(&[127, 0x80, 0, 0, 0, 0, 0, 0], true);
        assert_eq!((p[0], p[1]), (255, 0));
        let p = palette(&[0, 0, 0, 0, 0, 0, 0, 0], true);
        assert_eq!((p[0], p[6], p[7]), (128, 0, 255));
    }

    #[test]
    fn indices_and_partial_blocks() {
        // Endpoints 200 and 100, indices 0, 1, 2, ... (3 bits each, little-endian).
        let mut bits = 0u64;
        for i in 0..16u64 {
            bits |= (i % 8) << (3 * i);
        }
        let mut block = vec![200u8, 100];
        block.extend_from_slice(&bits.to_le_bytes()[..6]);
        let values = decode_bc4_block(&block, false);
        assert_eq!(&values[..3], &[200, 100, 186]);
        // A 3x2 image from one block keeps the top-left 3x2 texels.
        let rgba = decode(&block, 3, 2, 1, false).unwrap();
        let reds: Vec<u8> = rgba.chunks(4).map(|p| p[0]).collect();
        assert_eq!(
            reds,
            [
                values[0], values[1], values[2], values[4], values[5], values[6]
            ]
        );
        assert!(
            rgba.chunks(4)
                .all(|p| p[1] == 0 && p[2] == 0 && p[3] == 255)
        );
        assert!(decode(&block[..7], 3, 2, 1, false).is_none());
    }
}
