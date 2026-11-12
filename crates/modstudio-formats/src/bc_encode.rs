//! Block compression encoders: BC1 (with 1-bit alpha), BC3, BC4, BC5 and BC7 (mode 6).
//!
//! Endpoints start at the ends of the block's colours along their main axis and are then refined by least
//! squares against the chosen indices; each block is decoded the way [`crate::bcn`] and the GPU do to pick the
//! best candidate. Input is RGBA8, row by row; partial blocks at the edges repeat their last pixel.

/// 16 RGBA pixels of a 4x4 block (row by row), edges padded by repeating.
fn block_pixels(rgba: &[u8], width: usize, height: usize, bx: usize, by: usize) -> [[u8; 4]; 16] {
    let mut out = [[0u8; 4]; 16];
    for (i, px) in out.iter_mut().enumerate() {
        let x = (bx * 4 + i % 4).min(width - 1);
        let y = (by * 4 + i / 4).min(height - 1);
        let o = (y * width + x) * 4;
        px.copy_from_slice(&rgba[o..o + 4]);
    }
    out
}

/// Encodes a whole image: `encode_block` per 4x4 block, blocks row by row.
fn encode_image(
    rgba: &[u8],
    width: usize,
    height: usize,
    block_bytes: usize,
    encode_block: impl Fn(&[[u8; 4]; 16], &mut [u8]),
) -> Vec<u8> {
    let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
    let mut out = vec![0u8; bw * bh * block_bytes];
    for by in 0..bh {
        for bx in 0..bw {
            let px = block_pixels(rgba, width, height, bx, by);
            let at = (by * bw + bx) * block_bytes;
            encode_block(&px, &mut out[at..at + block_bytes]);
        }
    }
    out
}

// ---------------------------------------------------------------- BC4 / BC5

/// One 8-byte BC4 block for 16 values: the 8-value mode, endpoints refined, or the 6-value mode when the block
/// has exact 0 or 255 and does better that way.
fn bc4_block(values: &[u8; 16], out: &mut [u8]) {
    let lo = *values.iter().min().expect("16");
    let hi = *values.iter().max().expect("16");
    if lo == hi {
        out[0] = lo;
        out[1] = lo;
        out[2..8].fill(0);
        return;
    }
    let mut best = (u64::MAX, [0u8; 8]);
    let mut try_endpoints = |e0: u8, e1: u8| {
        let mut block = [0u8; 8];
        block[0] = e0;
        block[1] = e1;
        // The palette as decoded: a block whose indices are 0..7 in order.
        let pal: [u8; 8] = {
            let mut p = [0u8; 8];
            let mut probe = block;
            let mut bits = 0u64;
            for i in 0..8u64 {
                bits |= i << (3 * i);
            }
            probe[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
            let d = crate::bcn::decode_bc4_block(&probe, false);
            p.copy_from_slice(&d[..8]);
            p
        };
        let mut bits = 0u64;
        let mut err = 0u64;
        for (i, &v) in values.iter().enumerate() {
            let (k, e) = pal
                .iter()
                .enumerate()
                .map(|(k, &p)| (k, (i32::from(p) - i32::from(v)).unsigned_abs() as u64))
                .min_by_key(|&(_, e)| e)
                .expect("8");
            bits |= (k as u64) << (3 * i);
            err += e * e;
        }
        block[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
        if err < best.0 {
            best = (err, block);
        }
    };
    // 8-value mode (e0 > e1), the range and a slightly inset one.
    try_endpoints(hi, lo);
    let inset = ((hi - lo) / 16).max(1);
    if hi - lo > 2 * inset {
        try_endpoints(hi - inset, lo + inset);
    }
    // 6-value mode (e0 <= e1) around the values that aren't 0 or 255.
    let mid: Vec<u8> = values
        .iter()
        .copied()
        .filter(|&v| v != 0 && v != 255)
        .collect();
    if let (Some(&a), Some(&b)) = (mid.iter().min(), mid.iter().max()) {
        try_endpoints(a, b);
    }
    out.copy_from_slice(&best.1);
}

/// BC4 (one channel: red of each pixel).
pub fn encode_bc4(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    encode_image(rgba, width, height, 8, |px, out| {
        bc4_block(&px.map(|p| p[0]), out);
    })
}

/// BC5 (red and green).
pub fn encode_bc5(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    encode_image(rgba, width, height, 16, |px, out| {
        bc4_block(&px.map(|p| p[0]), &mut out[..8]);
        bc4_block(&px.map(|p| p[1]), &mut out[8..]);
    })
}

// ---------------------------------------------------------------- BC1 / BC3 colour

fn to565(c: [f32; 3]) -> u16 {
    let r = (c[0].clamp(0.0, 255.0) * 31.0 / 255.0).round() as u16;
    let g = (c[1].clamp(0.0, 255.0) * 63.0 / 255.0).round() as u16;
    let b = (c[2].clamp(0.0, 255.0) * 31.0 / 255.0).round() as u16;
    (r << 11) | (g << 5) | b
}

/// Principal axis of the colours (power iteration on the covariance), and their mean.
fn principal_axis(colors: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let n = colors.len().max(1) as f32;
    let mean = [0, 1, 2].map(|c| colors.iter().map(|p| p[c]).sum::<f32>() / n);
    let mut cov = [[0f32; 3]; 3];
    for p in colors {
        let d = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    let mut axis = [1.0f32, 1.0, 1.0];
    for _ in 0..8 {
        let next =
            [0, 1, 2].map(|i| cov[i][0] * axis[0] + cov[i][1] * axis[1] + cov[i][2] * axis[2]);
        let len = (next[0] * next[0] + next[1] * next[1] + next[2] * next[2]).sqrt();
        if len < 1e-9 {
            break;
        }
        axis = next.map(|v| v / len);
    }
    (mean, axis)
}

/// One 8-byte BC1-style colour block. `transparent`: pixels to make transparent black (3-colour mode).
fn bc1_block(px: &[[u8; 4]; 16], transparent: Option<&[bool; 16]>, out: &mut [u8]) {
    let use_alpha = transparent.is_some_and(|t| t.iter().any(|&x| x));
    let opaque: Vec<[f32; 3]> = (0..16)
        .filter(|&i| !transparent.is_some_and(|t| t[i]))
        .map(|i| {
            [
                f32::from(px[i][0]),
                f32::from(px[i][1]),
                f32::from(px[i][2]),
            ]
        })
        .collect();
    if opaque.is_empty() {
        // All transparent: 3-colour mode, every index 3.
        out[..4].copy_from_slice(&[0, 0, 0, 0]);
        out[4..8].copy_from_slice(&[0xFF; 4]);
        return;
    }
    let (mean, axis) = principal_axis(&opaque);
    let proj = |c: &[f32; 3]| {
        (c[0] - mean[0]) * axis[0] + (c[1] - mean[1]) * axis[1] + (c[2] - mean[2]) * axis[2]
    };
    let (lo, hi) = opaque
        .iter()
        .map(proj)
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), t| {
            (a.min(t), b.max(t))
        });
    let at = |t: f32| [0, 1, 2].map(|c| mean[c] + axis[c] * t);
    let mut candidates = vec![(at(hi), at(lo))];

    let mut best = (f32::INFINITY, [0u8; 8]);
    let evaluate = |c0: [f32; 3], c1: [f32; 3], best: &mut (f32, [u8; 8])| -> Option<[u8; 16]> {
        let (mut e0, mut e1) = (to565(c0), to565(c1));
        // Four-colour mode needs e0 > e1, three-colour (with alpha) e0 <= e1.
        if use_alpha {
            if e0 > e1 {
                std::mem::swap(&mut e0, &mut e1);
            }
        } else {
            if e0 < e1 {
                std::mem::swap(&mut e0, &mut e1);
            }
            if e0 == e1 {
                // A single colour: four-colour mode with e1 one step below (or above) still holds it.
                if e1 > 0 {
                    e1 -= 1;
                } else {
                    e0 += 1;
                }
                if e0 < e1 {
                    std::mem::swap(&mut e0, &mut e1);
                }
            }
        }
        let mut block = [0u8; 8];
        block[..2].copy_from_slice(&e0.to_le_bytes());
        block[2..4].copy_from_slice(&e1.to_le_bytes());
        let palette: [[u8; 4]; 4] = {
            let mut probe = block;
            probe[4..8].copy_from_slice(&0b11_10_01_00u32.to_le_bytes());
            let d = crate::bcn::decode_colour(&probe, 4, 1, crate::bcn::ColourBlocks::Bc1)
                .expect("one block");
            [0, 1, 2, 3].map(|k| [d[k * 4], d[k * 4 + 1], d[k * 4 + 2], d[k * 4 + 3]])
        };
        let mut indices = 0u32;
        let mut chosen = [0u8; 16];
        let mut err = 0f32;
        for i in 0..16 {
            let k = if transparent.is_some_and(|t| t[i]) {
                3
            } else {
                let usable = if use_alpha { 3 } else { 4 };
                (0..usable)
                    .min_by(|&a, &b| {
                        let d = |k: usize| {
                            (0..3)
                                .map(|c| (f32::from(palette[k][c]) - f32::from(px[i][c])).powi(2))
                                .sum::<f32>()
                        };
                        d(a).total_cmp(&d(b))
                    })
                    .expect("palette")
            };
            if !transparent.is_some_and(|t| t[i]) {
                err += (0..3)
                    .map(|c| (f32::from(palette[k][c]) - f32::from(px[i][c])).powi(2))
                    .sum::<f32>();
            }
            chosen[i] = k as u8;
            indices |= (k as u32) << (2 * i);
        }
        block[4..8].copy_from_slice(&indices.to_le_bytes());
        if err < best.0 {
            *best = (err, block);
        }
        Some(chosen)
    };

    // Refine: least squares endpoints for the chosen indices, a few rounds.
    let weights_of = |k: u8| -> f32 {
        if use_alpha {
            [0.0, 1.0, 0.5, 0.0][k as usize]
        } else {
            [0.0, 1.0, 1.0 / 3.0, 2.0 / 3.0][k as usize]
        }
    };
    let (c0, c1) = candidates.pop().expect("one");
    let mut pair = (c0, c1);
    for _ in 0..3 {
        let Some(chosen) = evaluate(pair.0, pair.1, &mut best) else {
            break;
        };
        // Solve for a, b in: colour = (1 - w) * a + w * b, over the opaque pixels.
        let e0 = u16::from_le_bytes([best.1[0], best.1[1]]);
        let e1 = u16::from_le_bytes([best.1[2], best.1[3]]);
        let swapped = if use_alpha { false } else { e0 < e1 };
        let (mut aa, mut ab, mut bb) = (0f32, 0f32, 0f32);
        let mut ax = [0f32; 3];
        let mut bx = [0f32; 3];
        for i in 0..16 {
            if transparent.is_some_and(|t| t[i]) {
                continue;
            }
            let mut w = weights_of(chosen[i]);
            if swapped {
                w = 1.0 - w;
            }
            let (wa, wb) = (1.0 - w, w);
            aa += wa * wa;
            ab += wa * wb;
            bb += wb * wb;
            for c in 0..3 {
                ax[c] += wa * f32::from(px[i][c]);
                bx[c] += wb * f32::from(px[i][c]);
            }
        }
        let det = aa * bb - ab * ab;
        if det.abs() < 1e-6 {
            break;
        }
        let a = [0, 1, 2].map(|c| (ax[c] * bb - bx[c] * ab) / det);
        let b = [0, 1, 2].map(|c| (bx[c] * aa - ax[c] * ab) / det);
        pair = (a, b);
    }
    out.copy_from_slice(&best.1);
}

/// BC1; pixels with alpha below 128 become transparent (BC1's 1-bit alpha).
pub fn encode_bc1(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    encode_image(rgba, width, height, 8, |px, out| {
        let transparent = px.map(|p| p[3] < 128);
        bc1_block(px, Some(&transparent), out);
    })
}

/// BC3: BC1 colour (four colours) and BC4 alpha.
pub fn encode_bc3(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    encode_image(rgba, width, height, 16, |px, out| {
        bc4_block(&px.map(|p| p[3]), &mut out[..8]);
        bc1_block(px, None, &mut out[8..]);
    })
}

// ---------------------------------------------------------------- BC7 mode 6

/// Bits written from the lowest bit up.
struct Bits {
    v: u128,
    at: u32,
}

impl Bits {
    fn put(&mut self, value: u32, n: u32) {
        self.v |= (u128::from(value) & ((1u128 << n) - 1)) << self.at;
        self.at += n;
    }
}

/// (error, endpoint 0, endpoint 1, p-bit 0, p-bit 1, indices)
type Mode6Try = (f32, [u32; 4], [u32; 4], u32, u32, [u32; 16]);

const BC7_WEIGHTS4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// One BC7 mode 6 block: one subset, RGBA endpoints of 7 bits plus a p-bit each, 4-bit indices.
fn bc7_mode6_block(px: &[[u8; 4]; 16], out: &mut [u8]) {
    let colors: Vec<[f32; 4]> = px.iter().map(|p| p.map(f32::from)).collect();
    // Main axis in RGBA.
    let mean = [0, 1, 2, 3].map(|c| colors.iter().map(|p| p[c]).sum::<f32>() / 16.0);
    let mut cov = [[0f32; 4]; 4];
    for p in &colors {
        let d = [0, 1, 2, 3].map(|c| p[c] - mean[c]);
        for i in 0..4 {
            for j in 0..4 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    let mut axis = [1.0f32, 1.0, 1.0, 0.5];
    for _ in 0..8 {
        let next = [0, 1, 2, 3].map(|i| (0..4).map(|j| cov[i][j] * axis[j]).sum::<f32>());
        let len = next.iter().map(|v| v * v).sum::<f32>().sqrt();
        if len < 1e-9 {
            break;
        }
        axis = next.map(|v| v / len);
    }
    let proj = |p: &[f32; 4]| (0..4).map(|c| (p[c] - mean[c]) * axis[c]).sum::<f32>();
    let (lo, hi) = colors
        .iter()
        .map(proj)
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), t| {
            (a.min(t), b.max(t))
        });
    let mut ends = (
        [0, 1, 2, 3].map(|c| mean[c] + axis[c] * lo),
        [0, 1, 2, 3].map(|c| mean[c] + axis[c] * hi),
    );

    let mut best: (f32, u128) = (f32::INFINITY, 0);
    for _round in 0..3 {
        // Quantise each endpoint with each p-bit; keep the best of the four combinations.
        let quant = |e: [f32; 4], p: u32| -> [u32; 4] {
            e.map(|v| {
                let q = ((v.clamp(0.0, 255.0) / 255.0 * 254.0 - p as f32) / 2.0)
                    .round()
                    .clamp(0.0, 127.0);
                q as u32
            })
        };
        let expand = |q: [u32; 4], p: u32| q.map(|v| ((v << 1) | p) as f32);
        let mut round_best: Option<Mode6Try> = None;
        for p0 in 0..2 {
            for p1 in 0..2 {
                let (q0, q1) = (quant(ends.0, p0), quant(ends.1, p1));
                let (a, b) = (expand(q0, p0), expand(q1, p1));
                let palette: Vec<[f32; 4]> = BC7_WEIGHTS4
                    .iter()
                    .map(|&w| {
                        [0, 1, 2, 3].map(|c| {
                            (((64 - w) as f32 * a[c] + w as f32 * b[c] + 32.0) / 64.0).floor()
                        })
                    })
                    .collect();
                let mut err = 0f32;
                let mut idx = [0u32; 16];
                for (i, p) in colors.iter().enumerate() {
                    let (k, e) = palette
                        .iter()
                        .enumerate()
                        .map(|(k, q)| (k, (0..4).map(|c| (q[c] - p[c]).powi(2)).sum::<f32>()))
                        .min_by(|x, y| x.1.total_cmp(&y.1))
                        .expect("16");
                    idx[i] = k as u32;
                    err += e;
                }
                if round_best.as_ref().is_none_or(|r| err < r.0) {
                    round_best = Some((err, q0, q1, p0, p1, idx));
                }
            }
        }
        let (err, mut q0, mut q1, mut p0, mut p1, mut idx) = round_best.expect("tried");
        // The anchor (first pixel's index) must have its top bit clear: swap the endpoints if not.
        if idx[0] & 8 != 0 {
            std::mem::swap(&mut q0, &mut q1);
            std::mem::swap(&mut p0, &mut p1);
            idx = idx.map(|k| 15 - k);
        }
        if err < best.0 {
            let mut bits = Bits { v: 0, at: 0 };
            bits.put(1 << 6, 7); // mode 6
            for c in 0..4 {
                bits.put(q0[c], 7);
                bits.put(q1[c], 7);
            }
            bits.put(p0, 1);
            bits.put(p1, 1);
            bits.put(idx[0], 3);
            for &k in &idx[1..] {
                bits.put(k, 4);
            }
            best = (err, bits.v);
        }
        // Least squares endpoints for these indices.
        let (mut aa, mut ab, mut bb) = (0f32, 0f32, 0f32);
        let (mut ax, mut bx) = ([0f32; 4], [0f32; 4]);
        for (i, p) in colors.iter().enumerate() {
            let w = BC7_WEIGHTS4[idx[i] as usize] as f32 / 64.0;
            let (wa, wb) = (1.0 - w, w);
            aa += wa * wa;
            ab += wa * wb;
            bb += wb * wb;
            for c in 0..4 {
                ax[c] += wa * p[c];
                bx[c] += wb * p[c];
            }
        }
        let det = aa * bb - ab * ab;
        if det.abs() < 1e-6 {
            break;
        }
        ends = (
            [0, 1, 2, 3].map(|c| (ax[c] * bb - bx[c] * ab) / det),
            [0, 1, 2, 3].map(|c| (bx[c] * aa - ax[c] * ab) / det),
        );
    }
    out.copy_from_slice(&best.1.to_le_bytes());
}

/// BC7, every block in mode 6.
pub fn encode_bc7(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    encode_image(rgba, width, height, 16, bc7_mode6_block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bcn::{ColourBlocks, decode, decode_colour};

    /// A test picture: smooth gradients, a hard edge and an alpha cut-out.
    fn picture(w: usize, h: usize) -> Vec<u8> {
        (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| {
                    let edge = if x > w / 2 { 60 } else { 0 };
                    [
                        (x * 255 / w) as u8,
                        (y * 255 / h) as u8,
                        ((x + y) * 127 / (w + h) + edge) as u8,
                        if (x / 3 + y / 5) % 4 == 0 { 0 } else { 255 },
                    ]
                })
            })
            .collect()
    }

    fn rmse(a: &[u8], b: &[u8], channels: &[usize]) -> f64 {
        let mut sum = 0f64;
        let mut n = 0f64;
        for (pa, pb) in a.chunks(4).zip(b.chunks(4)) {
            for &c in channels {
                sum += (f64::from(pa[c]) - f64::from(pb[c])).powi(2);
                n += 1.0;
            }
        }
        (sum / n).sqrt()
    }

    #[test]
    fn bc4_and_bc5_are_close() {
        let (w, h) = (37, 21);
        let img = picture(w, h);
        let dec = decode(&encode_bc4(&img, w, h), w, h, 1, false).unwrap();
        assert!(rmse(&img, &dec, &[0]) < 2.0, "{}", rmse(&img, &dec, &[0]));
        let dec = decode(&encode_bc5(&img, w, h), w, h, 2, false).unwrap();
        assert!(rmse(&img, &dec, &[0, 1]) < 2.0);
        // Flat blocks are exact.
        let flat = vec![200u8; 16 * 4];
        let dec = decode(&encode_bc4(&flat, 4, 4), 4, 4, 1, false).unwrap();
        assert!(dec.chunks(4).all(|p| p[0] == 200));
    }

    #[test]
    fn bc1_keeps_colours_and_cut_outs() {
        let (w, h) = (36, 20);
        let img = picture(w, h);
        let dec = decode_colour(&encode_bc1(&img, w, h), w, h, ColourBlocks::Bc1).unwrap();
        // Transparent where the picture is, colours close where it's opaque.
        for (a, b) in img.chunks(4).zip(dec.chunks(4)) {
            assert_eq!(a[3] < 128, b[3] == 0);
        }
        let opaque: (Vec<u8>, Vec<u8>) = img
            .chunks(4)
            .zip(dec.chunks(4))
            .filter(|(a, _)| a[3] >= 128)
            .fold((Vec::new(), Vec::new()), |(mut x, mut y), (a, b)| {
                x.extend_from_slice(a);
                y.extend_from_slice(b);
                (x, y)
            });
        assert!(
            rmse(&opaque.0, &opaque.1, &[0, 1, 2]) < 8.0,
            "{}",
            rmse(&opaque.0, &opaque.1, &[0, 1, 2])
        );
        let dec = decode_colour(&encode_bc3(&img, w, h), w, h, ColourBlocks::Bc3).unwrap();
        assert!(rmse(&img, &dec, &[3]) < 1.0);
        assert!(rmse(&img, &dec, &[0, 1, 2]) < 8.0);
    }

    #[test]
    fn bc7_mode6_is_close() {
        let (w, h) = (40, 24);
        let img = picture(w, h);
        let blocks = encode_bc7(&img, w, h);
        let mut words = vec![0u32; w * h];
        texture2ddecoder::decode_bc7(&blocks, w, h, &mut words).unwrap();
        let dec: Vec<u8> = words
            .iter()
            .flat_map(|v| {
                let [b, g, r, a] = v.to_le_bytes();
                [r, g, b, a]
            })
            .collect();
        assert!(
            rmse(&img, &dec, &[0, 1, 2, 3]) < 12.0,
            "{}",
            rmse(&img, &dec, &[0, 1, 2, 3])
        );
        // A flat colour comes back within the 7-bit + p-bit steps.
        let flat: Vec<u8> = [17u8, 133, 250, 255].repeat(16);
        let mut words = vec![0u32; 16];
        texture2ddecoder::decode_bc7(&encode_bc7(&flat, 4, 4), 4, 4, &mut words).unwrap();
        let [b, g, r, a] = words[0].to_le_bytes();
        for (x, y) in [r, g, b, a].iter().zip([17u8, 133, 250, 255]) {
            assert!(x.abs_diff(y) <= 1, "{:?}", [r, g, b, a]);
        }
    }
}
