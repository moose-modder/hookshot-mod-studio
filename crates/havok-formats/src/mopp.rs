//! MOPP code: the byte code of an `hkpMoppCode` (a bounding-volume tree over the triangles of an `hkpStorageExtendedMeshShape`,
//! Havok 7.1 on the PS3), walked for the shape keys it ends in ([`walk`]) and queried with a box ([`query`]).
//!
//! A code is a program for a small virtual machine. Its commands, with every multi-byte argument big endian (the opcodes
//! in the table are the ones the 1,705 codes of the 490 SHRED packfiles execute: 5,917,936 bytes, every code walked to the
//! end; the command table is the one soulstruct's MOPP reader documents, checked here against those codes):
//!
//! | opcode | command |
//! |---|---|
//! | `0x01`-`0x04` | rescale: three frame bytes `x y z`; the query box becomes `(q - (frame << 16)) << opcode` per axis |
//! | `0x05`, `0x06` | jump forward by the 8 or 16-bit argument that follows (from the end of the command) |
//! | `0x70` | jump to the absolute 32-bit address that follows |
//! | `0x09`, `0x0A` | add the 8 or 16-bit argument to the terminal offset |
//! | `0x0B` | set the terminal offset to the 32-bit argument |
//! | `0x10`-`0x12` | split on the x, y or z plane: `left_max right_min right_jump8`; the left branch (the next command) is followed when the box's minimum is at most `left_max`, the right (`jump8` on from the end of the command) when its maximum is at least `right_min` |
//! | `0x13`-`0x1C` | the same on a diagonal plane (followed on both sides when querying) |
//! | `0x20`-`0x22` | split on one value: `value right_jump8` |
//! | `0x23`-`0x25` | split with 16-bit jumps: `left_max right_min left_jump16 right_jump16` |
//! | `0x26`-`0x28` | bounds on the x, y or z axis: `min max`; a box outside them ends the path |
//! | `0x30`-`0x4F` | terminal: the shape key is the terminal offset plus the opcode minus `0x30` |
//! | `0x50`-`0x53` | terminal: the offset plus the following 8, 16, 24 or 32-bit value |
//!
//! The command table and the query semantics follow soulstruct-havok's `utilities/mopp.py` (Grimrukh, GPL-3.0-or-later),
//! which documents them for Demon's Souls and Dark Souls codes; this module re-checks them on SHRED's 7.1 codes.
//!
//! A command that none of the 1,705 codes executes is refused, with its opcode: `0x00` (return), `0x07` and `0x08` (longer
//! jumps), `0x0C` (a jump to a chunk), `0x29`-`0x2B` (bounds with 24-bit arguments) and everything else. The opcodes are
//! only the ones the data shows to be understood; the codes of other games that use them are not in the installs.
//!
//! A terminal is a shape key, `subpart << (32 - numBitsForSubpartIndex) | triangle` ([`shape_key`]; the bit count is 12 in
//! every SHRED mesh). A code omits the triangles its builder judged degenerate ([`is_degenerate`]), so for the 1,704 codes
//! over a mesh the terminals are the mesh's keys without those: [`check_mesh`] compares them.
//!
//! The query space is the box of the code: a world position `p` is the unsigned integer `floor((p - offset) * scale)` per
//! axis (`offset` and `scale` are the code's `info.offset`), and a command compares the high byte (`>> 16`) of the
//! query's coordinate in the current frame.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::{BTreeMap, BTreeSet};

use crate::packfile::physics::{ExtendedMesh, MoppCode};
use crate::{Result, invalid};

/// The most states ([`walk`]) or steps ([`query`]) a code may take before it is refused as a runaway: the longest of the
/// 1,705 SHRED codes (964,064 bytes) runs 304,222 commands in a walk.
const MAX_STEPS: usize = 1 << 24;

/// The bit count of the subpart index in every shape key of SHRED's extended meshes (`numBitsForSubpartIndex`).
pub const SUBPART_INDEX_BITS: u32 = 12;

/// A squared cross product below this makes a triangle degenerate ([`is_degenerate`]).
const DEGENERATE_CROSS_SQUARED: f32 = 1e-7;

/// One command, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    /// Rescale the query frame, then go on at `next`.
    Rescale { shift: u32, frame: [u8; 3], next: usize },
    /// Go on at `to`.
    Jump { to: usize },
    /// Add `by` to the terminal offset, then go on at `next`.
    AddOffset { by: u32, next: usize },
    /// Set the terminal offset, then go on at `next`.
    SetOffset { to: u32, next: usize },
    /// A split. `axis` is `None` on a diagonal plane.
    Split {
        axis: Option<usize>,
        left_max: u8,
        right_min: u8,
        left: usize,
        right: usize,
    },
    /// Bounds on one axis, then go on at `next`.
    Bounds {
        axis: usize,
        min: u8,
        max: u8,
        next: usize,
    },
    /// A terminal: the key is the terminal offset plus `add`.
    Terminal { add: u32 },
}

/// The big-endian value of the `len` bytes (at most four) at `at`.
fn be(code: &[u8], at: usize, len: usize) -> Result<u32> {
    let end = at
        .checked_add(len)
        .ok_or_else(|| invalid(at, "MOPP argument address overflow"))?;
    let bytes = code.get(at..end).ok_or_else(|| {
        invalid(
            at,
            format!(
                "a MOPP command runs past the end of the {} code bytes",
                code.len()
            ),
        )
    })?;
    Ok(bytes.iter().fold(0u32, |n, &b| (n << 8) | u32::from(b)))
}

/// `pc` moved on by `by` bytes.
fn forward(pc: usize, by: usize) -> Result<usize> {
    pc.checked_add(by)
        .ok_or_else(|| invalid(pc, "MOPP jump address overflow"))
}

/// `pc` moved on by `by` bytes, `by` as an argument value.
fn forward_by(pc: usize, by: u32) -> Result<usize> {
    forward(
        pc,
        usize::try_from(by).map_err(|_| invalid(pc, "MOPP jump exceeds the address space"))?,
    )
}

/// The command at `pc` and its opcode.
fn decode(code: &[u8], pc: usize) -> Result<(u8, Command)> {
    let op = *code
        .get(pc)
        .ok_or_else(|| invalid(pc, format!("MOPP execution leaves the {} code bytes", code.len())))?;
    let arg = |i: usize| -> Result<u32> { be(code, forward(pc, i)?, 1) };
    let command = match op {
        0x01..=0x04 => Command::Rescale {
            shift: u32::from(op),
            frame: [
                u8::try_from(arg(1)?).unwrap_or(0),
                u8::try_from(arg(2)?).unwrap_or(0),
                u8::try_from(arg(3)?).unwrap_or(0),
            ],
            next: forward(pc, 4)?,
        },
        0x05 | 0x06 => {
            // one byte of argument for 0x05, two for 0x06
            let size = usize::from(op.saturating_sub(4));
            let jump = be(code, forward(pc, 1)?, size)?;
            Command::Jump {
                to: forward_by(forward(pc, size.saturating_add(1))?, jump)?,
            }
        }
        0x70 => Command::Jump {
            to: usize::try_from(be(code, forward(pc, 1)?, 4)?)
                .map_err(|_| invalid(pc, "MOPP jump exceeds the address space"))?,
        },
        0x09 => Command::AddOffset {
            by: arg(1)?,
            next: forward(pc, 2)?,
        },
        0x0A => Command::AddOffset {
            by: be(code, forward(pc, 1)?, 2)?,
            next: forward(pc, 3)?,
        },
        0x0B => Command::SetOffset {
            to: be(code, forward(pc, 1)?, 4)?,
            next: forward(pc, 5)?,
        },
        0x10..=0x1C => Command::Split {
            axis: (op <= 0x12).then(|| usize::from(op.saturating_sub(0x10))),
            left_max: u8::try_from(arg(1)?).unwrap_or(0),
            right_min: u8::try_from(arg(2)?).unwrap_or(0),
            left: forward(pc, 4)?,
            right: forward_by(forward(pc, 4)?, arg(3)?)?,
        },
        0x20..=0x22 => {
            let value = u8::try_from(arg(1)?).unwrap_or(0);
            Command::Split {
                axis: Some(usize::from(op.saturating_sub(0x20))),
                left_max: value,
                right_min: value,
                left: forward(pc, 3)?,
                right: forward_by(forward(pc, 3)?, arg(2)?)?,
            }
        }
        0x23..=0x25 => {
            let end = forward(pc, 7)?;
            Command::Split {
                axis: Some(usize::from(op.saturating_sub(0x23))),
                left_max: u8::try_from(arg(1)?).unwrap_or(0),
                right_min: u8::try_from(arg(2)?).unwrap_or(0),
                left: forward_by(end, be(code, forward(pc, 3)?, 2)?)?,
                right: forward_by(end, be(code, forward(pc, 5)?, 2)?)?,
            }
        }
        0x26..=0x28 => Command::Bounds {
            axis: usize::from(op.saturating_sub(0x26)),
            min: u8::try_from(arg(1)?).unwrap_or(0),
            max: u8::try_from(arg(2)?).unwrap_or(0),
            next: forward(pc, 3)?,
        },
        0x30..=0x4F => Command::Terminal {
            add: u32::from(op.saturating_sub(0x30)),
        },
        0x50..=0x53 => Command::Terminal {
            // one to four bytes of value
            add: be(code, forward(pc, 1)?, usize::from(op.saturating_sub(0x4F)))?,
        },
        other => {
            return Err(invalid(
                pc,
                format!("MOPP opcode {other:#04x} does not occur in the codes this reader was checked on"),
            ));
        }
    };
    Ok((op, command))
}

/// The key of triangle `triangle` of subpart `subpart` when the shape keeps `subpart_bits` bits for the subpart: the
/// subpart in the high bits. `None` when either does not fit.
pub fn shape_key(subpart: usize, triangle: usize, subpart_bits: u32) -> Option<u32> {
    let shift = 32u32.checked_sub(subpart_bits)?;
    let subpart = u32::try_from(subpart).ok()?;
    let triangle = u32::try_from(triangle).ok()?;
    if subpart.checked_shr(subpart_bits)? != 0 || triangle.checked_shr(shift)? != 0 {
        return None;
    }
    Some(subpart.checked_shl(shift)? | triangle)
}

/// The subpart and triangle a shape key names (the inverse of [`shape_key`]).
pub fn split_shape_key(key: u32, subpart_bits: u32) -> Option<(usize, usize)> {
    let shift = 32u32.checked_sub(subpart_bits)?;
    let subpart = key.checked_shr(shift)?;
    let triangle = key & !subpart.checked_shl(shift)?;
    Some((usize::try_from(subpart).ok()?, usize::try_from(triangle).ok()?))
}

/// What walking every path of a code found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoppWalk {
    /// Every shape key some path ends in.
    pub terminals: BTreeSet<u32>,
    /// How many times each opcode was executed (a command reached again with another terminal offset counts again).
    pub opcodes: BTreeMap<u8, usize>,
}

/// Walk every path of `code`, ignoring the query: both branches of every split, every jump. A command is run once for
/// each terminal offset it is reached with, so a loop ends and a shared subtree is walked once per offset.
///
/// Refused: a command that runs past the code, a jump out of it, an opcode outside the table above, a key that
/// overflows 32 bits, a code that takes more than 2^24 steps. All 1,705 codes of the SHRED packfiles walk to the end.
pub fn walk(code: &[u8]) -> Result<MoppWalk> {
    let mut out = MoppWalk::default();
    let mut seen: BTreeSet<(usize, u32)> = BTreeSet::new();
    let mut paths: Vec<(usize, u32)> = vec![(0, 0)];
    while let Some((start, first_offset)) = paths.pop() {
        let (mut pc, mut offset) = (start, first_offset);
        loop {
            if !seen.insert((pc, offset)) {
                break;
            }
            if seen.len() > MAX_STEPS {
                return Err(invalid(pc, "a MOPP code takes more than 2^24 steps"));
            }
            let (op, command) = decode(code, pc)?;
            let count = out.opcodes.entry(op).or_insert(0);
            *count = count.saturating_add(1);
            match command {
                Command::Rescale { next, .. } | Command::Bounds { next, .. } => pc = next,
                Command::Jump { to } => pc = to,
                Command::AddOffset { by, next } => {
                    offset = offset
                        .checked_add(by)
                        .ok_or_else(|| invalid(pc, "a MOPP terminal offset overflows 32 bits"))?;
                    pc = next;
                }
                Command::SetOffset { to, next } => {
                    offset = to;
                    pc = next;
                }
                Command::Split { left, right, .. } => {
                    paths.push((right, offset));
                    pc = left;
                }
                Command::Terminal { add } => {
                    let key = offset
                        .checked_add(add)
                        .ok_or_else(|| invalid(pc, "a MOPP shape key overflows 32 bits"))?;
                    out.terminals.insert(key);
                    break;
                }
            }
        }
    }
    Ok(out)
}

/// The integer `floor((p - offset) * scale)` of one coordinate, as the code's query space has it.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the value is floored and clamped to the i64 range first"
)]
fn quantize(p: f32, offset: f32, scale: f32) -> i64 {
    let q = ((p - offset) * scale).floor();
    // `as` saturates at the i64 range and maps a NaN to 0; the inputs are checked finite
    q as i64
}

/// The shape keys of the terminals a query with `bounds` (minimum then maximum, world space) cannot rule out in `code`,
/// whose `info.offset` is `offset` with `scale`.
///
/// A box is turned into the code's integer space, and each split sends the query down the branches its high bytes
/// reach. Diagonal splits send it down both, so the result may hold keys a real query would not; it never loses one. A
/// path that reaches a terminal returns that terminal's key.
///
/// Refused as [`walk`] refuses, and a box that is not finite. For each of the 637,213 triangles of the 1,704 SHRED meshes
/// that a code has a terminal for, the query with the triangle's own box returns the triangle's key.
pub fn query(code: &[u8], offset: [f32; 3], scale: f32, bounds: [[f32; 3]; 2]) -> Result<BTreeSet<u32>> {
    if !scale.is_finite() || bounds.iter().chain([&offset]).flatten().any(|v| !v.is_finite()) {
        return Err(invalid(0, "a MOPP query needs a finite box and scale"));
    }
    let [low, high] = bounds;
    let mut lo = [0i64; 3];
    let mut hi = [0i64; 3];
    for axis in 0..3 {
        let (o, l, h) = (
            offset.get(axis).copied().unwrap_or(0.0),
            low.get(axis).copied().unwrap_or(0.0),
            high.get(axis).copied().unwrap_or(0.0),
        );
        if let (Some(a), Some(b)) = (lo.get_mut(axis), hi.get_mut(axis)) {
            *a = quantize(l, o, scale);
            *b = quantize(h, o, scale);
        }
    }
    let mut keys = BTreeSet::new();
    let mut steps = 0usize;
    let mut paths: Vec<(usize, u32, [i64; 3], [i64; 3])> = vec![(0, 0, lo, hi)];
    while let Some((start, first_offset, first_lo, first_hi)) = paths.pop() {
        let (mut pc, mut terminal_offset, mut lo, mut hi) = (start, first_offset, first_lo, first_hi);
        loop {
            steps = steps.saturating_add(1);
            if steps > MAX_STEPS {
                return Err(invalid(pc, "a MOPP query takes more than 2^24 steps"));
            }
            let (_, command) = decode(code, pc)?;
            match command {
                Command::Rescale { shift, frame, next } => {
                    let factor = 1i64.checked_shl(shift).unwrap_or(1);
                    let rescale = |v: [i64; 3]| -> [i64; 3] {
                        let mut out = v;
                        for (slot, f) in out.iter_mut().zip(frame) {
                            *slot = slot
                                .saturating_sub(i64::from(f).saturating_mul(65_536))
                                .saturating_mul(factor);
                        }
                        out
                    };
                    lo = rescale(lo);
                    hi = rescale(hi);
                    pc = next;
                }
                Command::Jump { to } => pc = to,
                Command::AddOffset { by, next } => {
                    terminal_offset = terminal_offset
                        .checked_add(by)
                        .ok_or_else(|| invalid(pc, "a MOPP terminal offset overflows 32 bits"))?;
                    pc = next;
                }
                Command::SetOffset { to, next } => {
                    terminal_offset = to;
                    pc = next;
                }
                Command::Split {
                    axis,
                    left_max,
                    right_min,
                    left,
                    right,
                } => {
                    let (go_left, go_right) = match axis {
                        Some(a) => (
                            lo.get(a).is_some_and(|v| (v >> 16) <= i64::from(left_max)),
                            hi.get(a).is_some_and(|v| (v >> 16) >= i64::from(right_min)),
                        ),
                        None => (true, true),
                    };
                    if go_right {
                        paths.push((right, terminal_offset, lo, hi));
                    }
                    if !go_left {
                        break;
                    }
                    pc = left;
                }
                Command::Bounds { axis, min, max, next } => {
                    let outside = hi.get(axis).is_some_and(|v| (v >> 16) < i64::from(min))
                        || lo.get(axis).is_some_and(|v| (v >> 16) > i64::from(max));
                    if outside {
                        break;
                    }
                    pc = next;
                }
                Command::Terminal { add } => {
                    let key = terminal_offset
                        .checked_add(add)
                        .ok_or_else(|| invalid(pc, "a MOPP shape key overflows 32 bits"))?;
                    keys.insert(key);
                    break;
                }
            }
        }
    }
    Ok(keys)
}

/// Whether a MOPP builder leaves the triangle `a b c` out: [`crate::is_degenerate_triangle`] with a threshold of `1e-7`
/// (the squared cross product of its edges, taken from either of its first two vertices, below the threshold, or the points
/// exactly collinear by the identity `|u|^2 |v|^2 - (u . v)^2 = 0` in `f32`, which long thin triangles reach by rounding).
/// Over the 1,704 SHRED codes that cover a mesh this separates all 8,760 triangles the codes have no terminal for from the
/// 637,213 they have one for (none of those passes it). The squared cross product alone separates 8,755: the other five, in
/// three meshes, are thin triangles of 6e-7 to 0.03 that the identity calls collinear, because the rounding in it
/// leaves exactly zero.
pub fn is_degenerate(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> bool {
    crate::degenerate::is_degenerate_triangle(a, b, c, DEGENERATE_CROSS_SQUARED)
}

/// How the terminals of a code compare with the triangles of the mesh under it ([`check_mesh`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoppCheck {
    /// The triangles of the mesh, over all subparts.
    pub triangles: usize,
    /// The terminals the code has.
    pub terminals: usize,
    /// Triangles that are degenerate by [`is_degenerate`].
    pub degenerate: usize,
    /// Terminals that name a triangle the mesh does not have.
    pub unknown_terminals: usize,
    /// Degenerate triangles the code has a terminal for anyway.
    pub degenerate_with_terminal: usize,
    /// The keys of triangles that are not degenerate by the rule and have no terminal, in key order.
    pub omitted: Vec<u32>,
}

/// Compare the terminals of `walk` with the triangles of `mesh`: a terminal is expected for every triangle that is not
/// degenerate, and for no other key. `subpart_bits` is the mesh's `numBitsForSubpartIndex` ([`SUBPART_INDEX_BITS`]).
///
/// Refused: a triangle that names a vertex the subpart lacks, or a key that does not fit the shape key.
pub fn check_mesh(walk: &MoppWalk, mesh: &ExtendedMesh, subpart_bits: u32) -> Result<MoppCheck> {
    let mut check = MoppCheck {
        terminals: walk.terminals.len(),
        ..MoppCheck::default()
    };
    let mut matched = 0usize;
    for (subpart_index, subpart) in mesh.subparts.iter().enumerate() {
        for (triangle_index, triangle) in subpart.triangles.iter().enumerate() {
            let key = shape_key(subpart_index, triangle_index, subpart_bits)
                .ok_or_else(|| invalid(0, "a triangle's shape key does not fit the key bits"))?;
            let corner = |i: u32| -> Result<[f32; 3]> {
                usize::try_from(i)
                    .ok()
                    .and_then(|i| subpart.vertices.get(i))
                    .copied()
                    .ok_or_else(|| invalid(0, format!("triangle {triangle_index} names vertex {i}")))
            };
            let [i0, i1, i2] = *triangle;
            let degenerate = is_degenerate(corner(i0)?, corner(i1)?, corner(i2)?);
            let present = walk.terminals.contains(&key);
            check.triangles = check.triangles.saturating_add(1);
            if present {
                matched = matched.saturating_add(1);
            }
            match (degenerate, present) {
                (true, true) => {
                    check.degenerate = check.degenerate.saturating_add(1);
                    check.degenerate_with_terminal = check.degenerate_with_terminal.saturating_add(1);
                }
                (true, false) => check.degenerate = check.degenerate.saturating_add(1),
                (false, false) => check.omitted.push(key),
                (false, true) => {}
            }
        }
    }
    check.unknown_terminals = walk.terminals.len().saturating_sub(matched);
    Ok(check)
}

/// Walk the code of `code` and compare it with `mesh` ([`walk`], [`check_mesh`]).
pub fn check_code(code: &MoppCode, mesh: &ExtendedMesh, subpart_bits: u32) -> Result<MoppCheck> {
    check_mesh(&walk(&code.code)?, mesh, subpart_bits)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "a test reads what it just built"
)]
mod tests {
    use super::*;

    #[test]
    fn shape_keys_put_the_subpart_in_the_high_bits() {
        assert_eq!(shape_key(0, 0, 12), Some(0));
        assert_eq!(shape_key(1, 0, 12), Some(1 << 20));
        assert_eq!(shape_key(3, 77, 12), Some(3 << 20 | 77));
        // the largest subpart and triangle that fit
        assert_eq!(shape_key(4095, (1 << 20) - 1, 12), Some(u32::MAX));
        assert_eq!(shape_key(4096, 0, 12), None);
        assert_eq!(shape_key(0, 1 << 20, 12), None);
        assert_eq!(split_shape_key(3 << 20 | 77, 12), Some((3, 77)));
        assert_eq!(split_shape_key(u32::MAX, 12), Some((4095, (1 << 20) - 1)));
    }

    #[test]
    fn a_split_walks_both_branches_and_a_terminal_ends_one() {
        // split on x at 7: the left branch is the next command (the one-byte terminal 0x33, key 3), the right one byte on
        // from the end of the split (terminal 0x50 with the value 200)
        let code = [0x20, 7, 1, 0x33, 0x50, 200];
        let walked = walk(&code).unwrap();
        assert_eq!(walked.terminals, BTreeSet::from([3, 200]));
        assert_eq!(walked.opcodes.get(&0x20), Some(&1));
        assert_eq!(walked.opcodes.get(&0x33), Some(&1));
        assert_eq!(walked.opcodes.get(&0x50), Some(&1));
    }

    #[test]
    fn terminal_offsets_add_and_set() {
        // offset += 5 (8-bit), += 0x0100 (16-bit), terminal 0x30 + 2
        let code = [0x09, 5, 0x0A, 0x01, 0x00, 0x32];
        assert_eq!(walk(&code).unwrap().terminals, BTreeSet::from([5 + 0x100 + 2]));
        // offset = 0x01020304, terminal with a 24-bit value
        let code = [0x0B, 1, 2, 3, 4, 0x52, 0, 0, 9];
        assert_eq!(walk(&code).unwrap().terminals, BTreeSet::from([0x0102_0304 + 9]));
    }

    #[test]
    fn jumps_move_the_program_counter() {
        // jump8 over two bytes to a terminal; jump16 and the absolute jump do the same
        let code = [0x05, 2, 0xcd, 0xcd, 0x31];
        assert_eq!(walk(&code).unwrap().terminals, BTreeSet::from([1]));
        let code = [0x06, 0, 1, 0xcd, 0x32];
        assert_eq!(walk(&code).unwrap().terminals, BTreeSet::from([2]));
        let code = [0x70, 0, 0, 0, 6, 0xcd, 0x33];
        assert_eq!(walk(&code).unwrap().terminals, BTreeSet::from([3]));
    }

    #[test]
    fn a_loop_ends_and_a_runaway_or_unknown_command_is_refused() {
        // jump back to itself: the same (pc, offset) is not run twice
        let code = [0x70, 0, 0, 0, 0];
        assert!(walk(&code).unwrap().terminals.is_empty());
        // a command cut off by the end of the code
        let e = walk(&[0x0A, 1]).unwrap_err().to_string();
        assert!(e.contains("runs past the end"), "{e}");
        // an opcode outside the table, and one the table lists as unseen
        for op in [0x00u8, 0x07, 0x0C, 0x29, 0xcd] {
            let e = walk(&[op, 0, 0, 0, 0, 0, 0, 0]).unwrap_err().to_string();
            assert!(e.contains("does not occur"), "{op:#x}: {e}");
        }
        // execution that walks off the end
        let e = walk(&[0x09, 1]).unwrap_err().to_string();
        assert!(e.contains("leaves the 2 code bytes"), "{e}");
    }

    #[test]
    fn a_query_follows_the_branches_its_box_reaches() {
        // x split at byte 7: left (terminal 1) when the box's minimum is at most 7, right (terminal 2) when its maximum
        // is at least 7; the right branch follows the left's one-byte terminal
        let code = [0x20, 7, 1, 0x31, 0x32];
        // query space: scale 1, offset 0 and a box of 0..=(8 << 16): high bytes 0 and 8
        let hi = (8u32 << 16) as f32;
        let both = query(&code, [0.0; 3], 1.0, [[0.0; 3], [hi, 0.0, 0.0]]).unwrap();
        assert_eq!(both, BTreeSet::from([1, 2]));
        let left = query(&code, [0.0; 3], 1.0, [[0.0; 3], [(6u32 << 16) as f32, 0.0, 0.0]]).unwrap();
        assert_eq!(left, BTreeSet::from([1]));
        let right = query(
            &code,
            [0.0; 3],
            1.0,
            [[(8u32 << 16) as f32, 0.0, 0.0], [(9u32 << 16) as f32, 0.0, 0.0]],
        )
        .unwrap();
        assert_eq!(right, BTreeSet::from([2]));
        // a box that is not finite is refused
        assert!(query(&code, [0.0; 3], 1.0, [[f32::NAN; 3], [0.0; 3]]).is_err());
    }

    #[test]
    fn a_rescale_zooms_the_frame_and_bounds_end_paths() {
        // rescale by 1 bit with frame x = 4: the box (8 << 16) becomes ((8 - 4) << 16) << 1 = 8 << 16
        // then a bounds check on x (min 8, max 9) passes it, and the terminal is key 7
        let code = [0x01, 4, 0, 0, 0x26, 8, 9, 0x37];
        let hit = query(
            &code,
            [0.0; 3],
            1.0,
            [[(8u32 << 16) as f32, 0.0, 0.0], [(8u32 << 16) as f32, 0.0, 0.0]],
        )
        .unwrap();
        assert_eq!(hit, BTreeSet::from([7]));
        // a box whose high byte is 5 after the rescale ((7 - 4) << 1 = 6) is outside 8..=9
        let miss = query(
            &code,
            [0.0; 3],
            1.0,
            [[(7u32 << 16) as f32, 0.0, 0.0], [(7u32 << 16) as f32, 0.0, 0.0]],
        )
        .unwrap();
        assert!(miss.is_empty());
    }

    #[test]
    fn a_degenerate_triangle_has_a_tiny_cross_product() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(!is_degenerate(a, b, c));
        // collinear
        assert!(is_degenerate(a, b, [2.0, 0.0, 0.0]));
        // a repeated vertex
        assert!(is_degenerate(a, a, c));
        // a sliver whose cross product squared is 1e-8
        assert!(is_degenerate(a, [1.0, 0.0, 0.0], [0.0, 1e-4, 0.0]));
        assert!(!is_degenerate(a, [1.0, 0.0, 0.0], [0.0, 1e-3, 0.0]));
    }
}
