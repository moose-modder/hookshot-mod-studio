//! Shapes stored as meshlets ([`shape_flags::MESHLET`](crate::bfres::shape_flags::MESHLET)): map pieces, objects,
//! cubemaps and a few NPC models. They have no meshes and an empty vertex buffer in the model; their geometry is
//! one GPU buffer of small clusters ("meshlets", up to 64 vertices and 124 triangles), each with its vertices
//! bit-packed at the precision it needs. Worked out from the game's files (no outside description of it is known);
//! what isn't understood is kept as it is (see [`Meshlet::record`]).
//!
//! The shape's meshlet record (at the pointer at shape + 0x18; [`MeshletBuffer`](crate::bfres::MeshletBuffer) finds
//! the buffer):
//!
//! | at   | what |
//! |------|------|
//! | 0x10 | pointer to the buffer infos (u32 size, alignment 0x40) |
//! | 0x18 | pointer to the buffer offsets (u64, from the start of the GPU data) |
//! | 0x20 | pointer to the triangles per LOD (u32) |
//! | 0x28 | pointer to the meshlets per LOD (u32) |
//! | 0x3c | u32 64 (vertices a meshlet may have, at most) |
//! | 0x40 | u32 124 (triangles a meshlet may have, at most) |
//! | 0x44 | u8 buffer count (1 in every file) |
//! | 0x45 | u8 LOD count |
//!
//! The buffer: a 0x54-byte header (the shape's bounding box minimum as 3 f32, the position step as f32 (0.0005 in
//! every file), [`flags`], then 16 u32 running totals of meshlets per LOD, the last repeated), the meshlets'
//! records, then each meshlet's data: its vertices (`vertex_bits` each, least significant bit first, no padding),
//! then 3 indices a triangle of just enough bits for the vertex count, padded to a byte.
//!
//! A meshlet's record: 40 bytes, then 8 for each UV set past the first and each colour set:
//!
//! | word | what |
//! |------|------|
//! | 0-3  | 128 bits: the position base x, y, z (21 bits each, in steps from the bounding box minimum), then twice the meshlet's extent x, y, z (21 bits each), then 2 bits (0) |
//! | 4    | bits 0-15 the normal cone axis (octahedral, 8+8; 0: no cone), 16-23 triangles, 24-31 vertices |
//! | 5    | bits 0-3 normal bits - 6, 4-31 where the data starts (from the end of the header) |
//! | 6    | bits 0-8 bits per vertex, 9-15 the normal cone cutoff (126: no cone), 16-20, 21-25, 26-30 position bits x, y, z, 31 (0) |
//! | 7    | 2 with tangents, 0 without |
//! | 8-9  | UV set 0: the base u, v (i16), then bits 0-11, 12-23 the u, v scale exponents `e` (4 to 7: steps of 2^(e-16)), 24-27 u bits - 1, 28-31 v bits - 1 |
//! | ...  | the other UV sets, alike |
//! | ...  | each colour set: a mask byte (low 4 bits: the channels that are constant, set; high 4 bits all set: the varying channels are 8-bit unorm, else f32), then the constant channels' values (a byte each) |
//!
//! A vertex: the position offsets x, y, z (from the base), the normal (octahedral, 2 x normal bits), with
//! [`flags::TANGENTS`] the tangent (1 bit: its sign, then octahedral 8+8), each colour set's varying channels, then
//! each UV set's offsets u, v (from the base).

use std::ops::Range;

use crate::bfres::Shape;
use crate::bytes::ReadLe;
use crate::error::{Result, invalid, unsupported};

/// The buffer header's flags.
pub mod flags {
    /// Vertices have tangents.
    pub const TANGENTS: u32 = 0x40;
    /// A colour set (`_c0`).
    pub const COLOUR_0: u32 = 0x400;
    /// A second colour set (`_c1`).
    pub const COLOUR_1: u32 = 0x800;
    /// A third UV set (after [`UV_1`]'s).
    pub const UV_2: u32 = 0x2000;
    /// A second UV set.
    pub const UV_1: u32 = 0x10000;
}

/// The header's size: where the records start.
pub const HEADER_SIZE: usize = 0x54;

/// A meshlet shape's buffer, read.
#[derive(Debug, Clone)]
pub struct MeshletShape<'a> {
    /// The buffer.
    pub data: &'a [u8],
    /// Where the buffer starts in the file.
    pub at: usize,
    pub bounds_min: [f32; 3],
    /// The position step.
    pub step: f32,
    pub flags: u32,
    /// Each LOD's meshlets, most detailed first.
    pub lods: Vec<Range<usize>>,
    pub meshlets: Vec<Meshlet>,
}

#[derive(Debug, Clone)]
pub struct Meshlet {
    /// The record's words, as they are.
    pub record: Vec<u32>,
    pub vertex_count: usize,
    pub triangle_count: usize,
    /// Where the vertices start, in bytes from the start of the buffer.
    pub data_at: usize,
    pub vertex_bits: u32,
    /// Bits of each of the normal's two octahedral components.
    pub normal_bits: u32,
    pub position_base: [u32; 3],
    pub position_bits: [u32; 3],
    pub uv_sets: Vec<UvSet>,
    pub colour_sets: Vec<ColourSet>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvSet {
    pub base: [i16; 2],
    /// u and v are in steps of 2^(exponent - 16).
    pub exponent: [u32; 2],
    pub bits: [u32; 2],
}

impl UvSet {
    /// The UV of offsets `u`, `v` from the base.
    pub fn uv(&self, u: u32, v: u32) -> [f32; 2] {
        let one = |c: usize, off: u32| {
            (self.base[c] as i64 + off as i64) as f32 * (2f32).powi(self.exponent[c] as i32 - 16)
        };
        [one(0, u), one(1, v)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColourSet {
    /// `_c<set>`.
    pub set: usize,
    pub channels: [Channel; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Channel {
    /// The same in every vertex of the meshlet: a byte (0..255 for 0..1).
    Constant(u8),
    /// A byte per vertex.
    Unorm8,
    /// An f32 per vertex.
    Float,
}

impl Channel {
    pub fn bits(self) -> u32 {
        match self {
            Channel::Constant(_) => 0,
            Channel::Unorm8 => 8,
            Channel::Float => 32,
        }
    }
}

/// A LOD's geometry: the meshlets' vertices one after another (a vertex two meshlets share is in both).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshletGeometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// xyz and the sign (w, +1 or -1) of the bitangent, cross(normal, tangent) * w (the way increasing v goes).
    pub tangents: Option<Vec<[f32; 4]>>,
    pub uvs: Vec<Vec<[f32; 2]>>,
    /// Each colour set's number and values.
    pub colours: Vec<(usize, Vec<[f32; 4]>)>,
    pub indices: Vec<u32>,
}

impl MeshletGeometry {
    /// The vertices, each once: vertices whose every value is the same (the ones two meshlets share) made one, in
    /// the order they first come.
    pub fn welded(&self) -> MeshletGeometry {
        let mut out = MeshletGeometry {
            tangents: self.tangents.as_ref().map(|_| Vec::new()),
            uvs: vec![Vec::new(); self.uvs.len()],
            colours: self
                .colours
                .iter()
                .map(|(set, _)| (*set, Vec::new()))
                .collect(),
            ..Default::default()
        };
        let mut seen: std::collections::HashMap<Vec<u32>, u32> = std::collections::HashMap::new();
        let mut remap = Vec::with_capacity(self.positions.len());
        for v in 0..self.positions.len() {
            let mut key: Vec<u32> = self.positions[v]
                .iter()
                .chain(&self.normals[v])
                .map(|x| x.to_bits())
                .collect();
            if let Some(t) = &self.tangents {
                key.extend(t[v].iter().map(|x| x.to_bits()));
            }
            for (_, c) in &self.colours {
                key.extend(c[v].iter().map(|x| x.to_bits()));
            }
            for uv in &self.uvs {
                key.extend(uv[v].iter().map(|x| x.to_bits()));
            }
            let next = out.positions.len() as u32;
            let index = *seen.entry(key).or_insert(next);
            if index == next {
                out.positions.push(self.positions[v]);
                out.normals.push(self.normals[v]);
                if let (Some(o), Some(t)) = (&mut out.tangents, &self.tangents) {
                    o.push(t[v]);
                }
                for (o, (_, c)) in out.colours.iter_mut().zip(&self.colours) {
                    o.1.push(c[v]);
                }
                for (o, uv) in out.uvs.iter_mut().zip(&self.uvs) {
                    o.push(uv[v]);
                }
            }
            remap.push(index);
        }
        out.indices = self.indices.iter().map(|&i| remap[i as usize]).collect();
        out
    }
}

/// `width` (at most 32) bits from bit `at`, least significant first; bits past the end read as 0.
pub fn bits(data: &[u8], at: usize, width: u32) -> u32 {
    debug_assert!(width <= 32);
    if width == 0 {
        return 0;
    }
    let byte = at / 8;
    let mut word = [0u8; 8];
    if let Some(rest) = data.get(byte..) {
        let n = rest.len().min(8);
        word[..n].copy_from_slice(&rest[..n]);
    }
    let v = u64::from_le_bytes(word) >> (at % 8);
    (v & ((1u64 << width) - 1)) as u32
}

/// Bits of each index in a meshlet of `vertices` vertices.
pub fn index_bits(vertices: usize) -> u32 {
    (usize::BITS - vertices.max(2).saturating_sub(1).leading_zeros()).max(1)
}

/// The octahedral direction of `x`, `y` (each 0..1).
pub fn octahedral(x: f32, y: f32) -> [f32; 3] {
    let (mut x, mut y) = (x * 2.0 - 1.0, y * 2.0 - 1.0);
    let z = 1.0 - x.abs() - y.abs();
    if z < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * ox.signum();
        y = (1.0 - ox.abs()) * oy.signum();
    }
    let l = (x * x + y * y + z * z).sqrt();
    if l == 0.0 {
        return [0.0, 0.0, 1.0];
    }
    [x / l, y / l, z / l]
}

fn unorm(v: u32, bits: u32) -> f32 {
    if bits == 0 {
        return 0.0;
    }
    v as f32 / ((1u64 << bits) - 1) as f32
}

/// The size of a meshlet record with these flags.
pub fn record_size(flags: u32) -> usize {
    let extra = [flags::UV_1, flags::UV_2, flags::COLOUR_0, flags::COLOUR_1];
    40 + 8 * extra.iter().filter(|&&f| flags & f != 0).count()
}

fn uv_set_count(flags: u32) -> usize {
    1 + (flags & flags::UV_1 != 0) as usize + (flags & flags::UV_2 != 0) as usize
}

impl Meshlet {
    fn parse(flags: u32, record: Vec<u32>) -> Meshlet {
        let w = &record;
        let big = w[0] as u128 | (w[1] as u128) << 32 | (w[2] as u128) << 64 | (w[3] as u128) << 96;
        let field = |shift: u32| ((big >> shift) & 0x1f_ffff) as u32;
        let uv_sets: Vec<UvSet> = (0..uv_set_count(flags))
            .map(|k| {
                let (base, form) = (w[8 + 2 * k], w[9 + 2 * k]);
                UvSet {
                    base: [base as u16 as i16, (base >> 16) as u16 as i16],
                    exponent: [form & 0xfff, (form >> 12) & 0xfff],
                    bits: [((form >> 24) & 15) + 1, ((form >> 28) & 15) + 1],
                }
            })
            .collect();
        let mut at = 8 + 2 * uv_sets.len();
        let mut colour_sets = Vec::new();
        for (set, flag) in [(0, flags::COLOUR_0), (1, flags::COLOUR_1)] {
            if flags & flag == 0 {
                continue;
            }
            let (mask, constants) = (w[at] & 0xff, w[at + 1]);
            let wide = mask >> 4 != 0xf;
            let channels = std::array::from_fn(|c| {
                if (mask >> c) & 1 != 0 {
                    Channel::Constant((constants >> (8 * c)) as u8)
                } else if wide {
                    Channel::Float
                } else {
                    Channel::Unorm8
                }
            });
            colour_sets.push(ColourSet { set, channels });
            at += 2;
        }
        Meshlet {
            vertex_count: (w[4] >> 24) as usize,
            triangle_count: ((w[4] >> 16) & 0xff) as usize,
            data_at: HEADER_SIZE + (w[5] >> 4) as usize,
            vertex_bits: w[6] & 0x1ff,
            normal_bits: (w[5] & 15) + 6,
            position_base: [field(0), field(21), field(42)],
            position_bits: [(w[6] >> 16) & 31, (w[6] >> 21) & 31, (w[6] >> 26) & 31],
            uv_sets,
            colour_sets,
            record,
        }
    }

    /// The bits the vertex fields take: `vertex_bits` when the layout is as understood.
    pub fn field_bits(&self, flags: u32) -> u32 {
        self.position_bits.iter().sum::<u32>()
            + 2 * self.normal_bits
            + if flags & flags::TANGENTS != 0 { 17 } else { 0 }
            + self
                .colour_sets
                .iter()
                .flat_map(|c| c.channels)
                .map(Channel::bits)
                .sum::<u32>()
            + self
                .uv_sets
                .iter()
                .map(|u| u.bits[0] + u.bits[1])
                .sum::<u32>()
    }

    /// The bit where the indices start, and the bit after them.
    pub fn index_span(&self) -> (usize, usize) {
        let start = self.data_at * 8 + self.vertex_count * self.vertex_bits as usize;
        (
            start,
            start + self.triangle_count * 3 * index_bits(self.vertex_count) as usize,
        )
    }

    /// The triangles' vertex indices (in this meshlet).
    pub fn indices(&self, data: &[u8]) -> Vec<u32> {
        let ib = index_bits(self.vertex_count);
        let (start, _) = self.index_span();
        (0..self.triangle_count * 3)
            .map(|k| bits(data, start + k * ib as usize, ib))
            .collect()
    }
}

impl<'a> MeshletShape<'a> {
    /// A meshlet shape's buffer.
    pub fn read(shape: &Shape<'a>) -> Result<MeshletShape<'a>> {
        let b = shape
            .meshlet
            .ok_or_else(|| invalid(format!("{}: not stored as meshlets", shape.name)))?;
        Self::parse(b.data, b.at, b.lods.clamp(1, 16))
            .map_err(|e| invalid(format!("{}: {e}", shape.name)))
    }

    /// A buffer (`at`: where it is in the file) with `lod_count` LODs.
    pub fn parse(data: &'a [u8], at: usize, lod_count: usize) -> Result<MeshletShape<'a>> {
        let bounds_min = [data.f32_at(0)?, data.f32_at(4)?, data.f32_at(8)?];
        let step = data.f32_at(12)?;
        let flags = data.u32_at(16)?;
        let mut lods = Vec::new();
        let mut start = 0;
        for k in 0..lod_count {
            let end = data.u32_at(0x14 + 4 * k)? as usize;
            if end < start {
                return Err(invalid(format!(
                    "LOD {k} ends at meshlet {end}, before {start}"
                )));
            }
            lods.push(start..end);
            start = end;
        }
        let size = record_size(flags);
        let meshlets = (0..start)
            .map(|i| {
                let at = HEADER_SIZE + i * size;
                let record = (0..size / 4)
                    .map(|k| data.u32_at(at + 4 * k))
                    .collect::<Result<Vec<u32>>>()?;
                Ok(Meshlet::parse(flags, record))
            })
            .collect::<Result<Vec<Meshlet>>>()?;
        for (i, m) in meshlets.iter().enumerate() {
            let fields = m.field_bits(flags);
            if fields != m.vertex_bits {
                return Err(unsupported(format!(
                    "meshlet {i}: its vertex fields take {fields} bits of {} (flags 0x{flags:x})",
                    m.vertex_bits
                )));
            }
            if m.index_span().1 > data.len() * 8 {
                return Err(invalid(format!(
                    "meshlet {i}: its data runs past the buffer"
                )));
            }
        }
        Ok(MeshletShape {
            data,
            at,
            bounds_min,
            step,
            flags,
            lods,
            meshlets,
        })
    }

    /// LOD `lod`'s geometry (a LOD past the last: the last).
    pub fn geometry(&self, lod: usize) -> Result<MeshletGeometry> {
        let range = self
            .lods
            .get(lod.min(self.lods.len().saturating_sub(1)))
            .cloned()
            .unwrap_or(0..0);
        let tangents = self.flags & flags::TANGENTS != 0;
        let mut g = MeshletGeometry {
            tangents: tangents.then(Vec::new),
            ..Default::default()
        };
        let first = range.clone().next().map(|i| &self.meshlets[i]);
        g.uvs = vec![Vec::new(); first.map_or(0, |m| m.uv_sets.len())];
        g.colours = first
            .map(|m| m.colour_sets.iter().map(|c| (c.set, Vec::new())).collect())
            .unwrap_or_default();
        for i in range {
            let m = &self.meshlets[i];
            let first_vertex = g.positions.len() as u32;
            for v in 0..m.vertex_count {
                let mut at = m.data_at * 8 + v * m.vertex_bits as usize;
                let mut take = |width: u32| {
                    let x = bits(self.data, at, width);
                    at += width as usize;
                    x
                };
                let mut p = [0f32; 3];
                for (c, p) in p.iter_mut().enumerate() {
                    let off = take(m.position_bits[c]);
                    *p = self.bounds_min[c] + (m.position_base[c] + off) as f32 * self.step;
                }
                g.positions.push(p);
                let (nx, ny) = (take(m.normal_bits), take(m.normal_bits));
                g.normals.push(octahedral(
                    unorm(nx, m.normal_bits),
                    unorm(ny, m.normal_bits),
                ));
                if let Some(t) = &mut g.tangents {
                    let sign = take(1);
                    let (tx, ty) = (take(8), take(8));
                    let [x, y, z] = octahedral(unorm(tx, 8), unorm(ty, 8));
                    t.push([x, y, z, if sign != 0 { 1.0 } else { -1.0 }]);
                }
                for (k, c) in m.colour_sets.iter().enumerate() {
                    let mut value = [0f32; 4];
                    for (ch, out) in c.channels.iter().zip(value.iter_mut()) {
                        *out = match *ch {
                            Channel::Constant(b) => b as f32 / 255.0,
                            Channel::Unorm8 => take(8) as f32 / 255.0,
                            Channel::Float => f32::from_bits(take(32)),
                        };
                    }
                    if let Some((_, values)) = g.colours.get_mut(k) {
                        values.push(value);
                    }
                }
                for (k, u) in m.uv_sets.iter().enumerate() {
                    let (du, dv) = (take(u.bits[0]), take(u.bits[1]));
                    if let Some(values) = g.uvs.get_mut(k) {
                        values.push(u.uv(du, dv));
                    }
                }
            }
            for ix in m.indices(self.data) {
                if ix as usize >= m.vertex_count {
                    return Err(invalid(format!(
                        "meshlet {i}: index {ix} of {} vertices",
                        m.vertex_count
                    )));
                }
                g.indices.push(first_vertex + ix);
            }
        }
        Ok(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bits written least significant first.
    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        at: usize,
    }

    impl BitWriter {
        fn put(&mut self, v: u32, width: u32) {
            for k in 0..width {
                if self.at / 8 >= self.bytes.len() {
                    self.bytes.push(0);
                }
                if (v >> k) & 1 != 0 {
                    self.bytes[self.at / 8] |= 1 << (self.at % 8);
                }
                self.at += 1;
            }
        }
    }

    #[test]
    fn bits_read_least_significant_first() {
        let d = [0b1010_1100u8, 0b0000_0011, 0xff];
        assert_eq!(bits(&d, 2, 4), 0b1011);
        assert_eq!(bits(&d, 6, 4), 0b1110);
        assert_eq!(bits(&d, 20, 8), 0x0f, "past the end reads 0");
        assert_eq!(index_bits(64), 6);
        assert_eq!(index_bits(65), 7);
        assert_eq!(index_bits(3), 2);
        assert_eq!(index_bits(1), 1);
    }

    #[test]
    fn octahedral_axes() {
        assert_eq!(octahedral(0.5, 0.5), [0.0, 0.0, 1.0]);
        assert_eq!(octahedral(1.0, 0.5), [1.0, 0.0, 0.0]);
        let down = octahedral(0.0, 0.0);
        assert!((down[2] + 1.0).abs() < 1e-6, "{down:?}");
    }

    /// One meshlet (a triangle) with tangents, a colour set and two UV sets, built bit by bit.
    fn one_meshlet() -> Vec<u8> {
        let flags = 0x1061 | flags::UV_1 | flags::COLOUR_0;
        let (px, py, pz, nb) = (3u32, 2u32, 4u32, 7u32);
        // colour set 0: channels 0 and 3 vary (8 bits), 1 and 2 constant 0xff
        let colour = [0xf6u32, 0x00ff_ff00];
        let vbits = px + py + pz + 2 * nb + 17 + 16 + (4 + 5) + (3 + 3);
        let mut w = vec![0u32; record_size(flags) / 4];
        let base = [10u128, 20, 30];
        let big = base[0] | base[1] << 21 | base[2] << 42;
        w[0] = big as u32;
        w[1] = (big >> 32) as u32;
        w[2] = (big >> 64) as u32;
        w[4] = 3 << 24 | 1 << 16;
        w[5] = ((record_size(flags) as u32) << 4) | (nb - 6);
        w[6] = vbits | 0x7e << 9 | px << 16 | py << 21 | pz << 26;
        w[7] = 2;
        w[8] = (-40i16 as u16 as u32) << 16 | 100;
        w[9] = (4 - 1) << 24 | (5 - 1) << 28 | 0x4004;
        w[10] = 7 << 16 | 8;
        w[11] = (3 - 1) << 24 | (3 - 1) << 28 | 0x5004;
        w[12] = colour[0];
        w[13] = colour[1];
        let mut out = Vec::new();
        for f in [0.0f32, 1.0, 2.0, 0.5] {
            out.extend(f.to_le_bytes());
        }
        out.extend(flags.to_le_bytes());
        for _ in 0..16 {
            out.extend(1u32.to_le_bytes());
        }
        assert_eq!(out.len(), HEADER_SIZE);
        for x in &w {
            out.extend(x.to_le_bytes());
        }
        let mut b = BitWriter::default();
        let up = (1u32 << nb) - 1; // normal +z: both components at the middle
        for v in 0..3u32 {
            b.put(v, px);
            b.put(v % 4, py);
            b.put(v * 2, pz);
            b.put(up / 2 + 1, nb);
            b.put(up / 2 + 1, nb);
            b.put(v & 1, 1); // tangent sign
            b.put(255, 8); // tangent +x
            b.put(128, 8);
            b.put(v * 50, 8); // colour channel 0
            b.put(255 - v, 8); // colour channel 3
            b.put(v * 5, 4); // uv0
            b.put(v * 9, 5);
            b.put(v, 3); // uv1
            b.put(7 - v, 3);
        }
        for ix in [0u32, 1, 2] {
            b.put(ix, index_bits(3));
        }
        out.extend(b.bytes);
        out
    }

    #[test]
    fn a_meshlet_decodes() {
        let data = one_meshlet();
        let s = MeshletShape::parse(&data, 0, 1).unwrap();
        assert_eq!(s.lods, vec![0..1]);
        let m = &s.meshlets[0];
        assert_eq!((m.vertex_count, m.triangle_count, m.normal_bits), (3, 1, 7));
        assert_eq!(m.position_base, [10, 20, 30]);
        assert_eq!(
            m.colour_sets[0].channels,
            [
                Channel::Unorm8,
                Channel::Constant(0xff),
                Channel::Constant(0xff),
                Channel::Unorm8
            ]
        );
        let g = s.geometry(0).unwrap();
        assert_eq!(g.indices, vec![0, 1, 2]);
        // positions: the minimum plus (base + offset) steps of 0.5
        assert_eq!(
            g.positions[2],
            [0.0 + 12.0 * 0.5, 1.0 + 22.0 * 0.5, 2.0 + 34.0 * 0.5]
        );
        for n in &g.normals {
            assert!(n[2] > 0.99, "{n:?}");
        }
        let t = g.tangents.as_ref().unwrap();
        assert!(t[0][0] > 0.99 && t[0][3] == -1.0 && t[1][3] == 1.0, "{t:?}");
        assert_eq!(g.colours[0].0, 0);
        assert_eq!(g.colours[0].1[2], [100.0 / 255.0, 1.0, 1.0, 253.0 / 255.0]);
        assert_eq!(g.uvs.len(), 2);
        assert_eq!(
            g.uvs[0][2],
            [(100.0 + 10.0) / 4096.0, (-40.0 + 18.0) / 4096.0]
        );
        assert_eq!(g.uvs[1][1], [(8.0 + 1.0) / 4096.0, (7.0 + 6.0) / 2048.0]);
    }

    #[test]
    fn shared_vertices_are_welded() {
        let g = MeshletGeometry {
            positions: vec![
                [0.0; 3],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0; 3],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 6],
            uvs: vec![vec![
                [0.0, 0.0],
                [1.0, 0.0],
                [0.0, 1.0],
                [1.0, 0.0],
                [0.5, 1.0],
                [1.0, 1.0],
            ]],
            indices: vec![0, 1, 2, 3, 4, 5],
            ..Default::default()
        };
        let w = g.welded();
        // vertex 3 is vertex 1 again; vertex 4 sits where 2 is but its UV differs (a seam): kept
        assert_eq!(w.positions.len(), 5);
        assert_eq!(w.indices, vec![0, 1, 2, 1, 3, 4]);
        assert_eq!(w.uvs[0][3], [0.5, 1.0]);
    }

    #[test]
    fn a_layout_that_doesnt_add_up_is_refused() {
        let mut data = one_meshlet();
        // one more bit per vertex than the fields take
        let w6 = HEADER_SIZE + 6 * 4;
        data[w6] += 1;
        let e = MeshletShape::parse(&data, 0, 1).unwrap_err().to_string();
        assert!(e.contains("vertex fields take"), "{e}");
    }
}
