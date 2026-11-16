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
//!
//! [`encode`] writes a buffer the way the game's are (checked against all of them, `examples/meshlet_census.rs`): the
//! header's minimum the vertices' (all LODs), each meshlet's bases its smallest values and its widths the bit length
//! of its largest offsets (UVs at least 1 bit), a UV component's scale the finest of 2^-12 .. 2^-9 its values fit, a
//! colour channel constant when every vertex has the same byte and 8-bit unless a channel leaves 0..1 (then every
//! varying channel f32), the data blocks one after another, the buffer padded to 0x100 bytes. Chosen its own way: the
//! meshlets (triangles grouped by the vertices they share and where they are), the normals' bits (the fewest from 6
//! that keep each within half a degree), and no normal cones (two meshlets in three in the game's files have none).

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

/// UV sets the vertices of a buffer with these flags have.
pub fn uv_set_count(flags: u32) -> usize {
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

/// The position step of every game file.
pub const STEP: f32 = 0.0005;
/// Vertices a meshlet may have, at most (the shape record's +0x3c).
pub const MAX_VERTICES: usize = 64;
/// Triangles a meshlet may have, at most (the shape record's +0x40).
pub const MAX_TRIANGLES: usize = 124;
/// The normal cone cutoff of a meshlet without a cone.
const NO_CONE: u32 = 126;
/// A normal is written with the fewest bits (from 6, at most 12, the game's range) that keep it this close.
const NORMAL_ERROR_DEGREES: f32 = 0.5;

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

fn bit_length(x: u64) -> u32 {
    u64::BITS - x.leading_zeros()
}

fn normalize(n: [f32; 3]) -> [f32; 3] {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l > 0.0 && l.is_finite() {
        [n[0] / l, n[1] / l, n[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// `n` as octahedral components of `bits` bits each: of the four lattice points around it, the nearest.
pub fn octahedral_encode(n: [f32; 3], bits: u32) -> [u32; 2] {
    let max = ((1u64 << bits) - 1) as f32;
    let l1 = n[0].abs() + n[1].abs() + n[2].abs();
    if !(l1 > 0.0 && l1.is_finite()) {
        let mid = (max / 2.0).round() as u32;
        return [mid, mid];
    }
    let sign = |x: f32| if x >= 0.0 { 1.0 } else { -1.0 };
    let (mut x, mut y) = (n[0] / l1, n[1] / l1);
    if n[2] < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * sign(ox);
        y = (1.0 - ox.abs()) * sign(oy);
    }
    let (u, v) = ((x * 0.5 + 0.5) * max, (y * 0.5 + 0.5) * max);
    let unit = normalize(n);
    let mut best = ([0u32; 2], f32::MIN);
    for a in [u.floor(), u.ceil()] {
        for b in [v.floor(), v.ceil()] {
            let (a, b) = (a.clamp(0.0, max), b.clamp(0.0, max));
            let d = octahedral(a / max, b / max);
            let cos = d[0] * unit[0] + d[1] * unit[1] + d[2] * unit[2];
            if cos > best.1 {
                best = ([a as u32, b as u32], cos);
            }
        }
    }
    best.0
}

/// A buffer written by [`encode`].
#[derive(Debug, Clone, PartialEq)]
pub struct Encoded {
    /// The buffer, padded to 0x100 bytes.
    pub data: Vec<u8>,
    pub bounds_min: [f32; 3],
    /// Meshlets of each LOD.
    pub lod_meshlets: Vec<u32>,
    /// Triangles of each LOD.
    pub lod_triangles: Vec<u32>,
    /// The vertices' positions as written (every LOD, on the step grid), for the shape's bounds.
    pub positions: Vec<[f32; 3]>,
}

/// A LOD's triangles grouped into meshlets of at most [`MAX_VERTICES`] vertices and [`MAX_TRIANGLES`] triangles:
/// each meshlet's vertices (indices into the LOD's, in the order its triangles first use them) and its triangles
/// (indices into those). A meshlet grows by the triangle around its vertices that brings the fewest new ones (then
/// the one nearest its middle); when none is left it goes on with the first unused triangle in a Z-order of the
/// triangles' middles, so a meshlet stays in one place. No meshlet is more than `max_extent` across on any axis.
/// Triangles that use a vertex twice are left out.
pub fn build_meshlets(
    positions: &[[f32; 3]],
    indices: &[u32],
    max_extent: f32,
) -> Vec<(Vec<u32>, Vec<u8>)> {
    const NONE: u8 = u8::MAX;
    let tris: Vec<[u32; 3]> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .copied()
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect();
    if tris.is_empty() {
        return Vec::new();
    }
    let vertex_count = positions.len();
    let centres: Vec<[f32; 3]> = tris
        .iter()
        .map(|t| {
            std::array::from_fn(|k| t.iter().map(|&v| positions[v as usize][k]).sum::<f32>() / 3.0)
        })
        .collect();
    // the Z-order of the triangles' middles
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for c in &centres {
        for k in 0..3 {
            lo[k] = lo[k].min(c[k]);
            hi[k] = hi[k].max(c[k]);
        }
    }
    let morton = |c: &[f32; 3]| -> u64 {
        let mut code = 0u64;
        for k in 0..3 {
            let spread = if hi[k] > lo[k] { hi[k] - lo[k] } else { 1.0 };
            let q = (((c[k] - lo[k]) / spread) * 1023.0).clamp(0.0, 1023.0) as u64;
            for bit in 0..10 {
                code |= ((q >> bit) & 1) << (3 * bit + k);
            }
        }
        code
    };
    let mut order: Vec<usize> = (0..tris.len()).collect();
    order.sort_by_key(|&t| (morton(&centres[t]), t));
    // the triangles around each vertex
    let mut start = vec![0usize; vertex_count + 1];
    for t in &tris {
        for &v in t {
            start[v as usize + 1] += 1;
        }
    }
    for v in 0..vertex_count {
        start[v + 1] += start[v];
    }
    let mut around = vec![0usize; start[vertex_count]];
    let mut fill = start.clone();
    for (i, t) in tris.iter().enumerate() {
        for &v in t {
            around[fill[v as usize]] = i;
            fill[v as usize] += 1;
        }
    }

    let mut local = vec![NONE; vertex_count];
    let mut used = vec![false; tris.len()];
    let mut cursor = 0usize;
    let mut out = Vec::new();
    let mut verts: Vec<u32> = Vec::new();
    let mut local_tris: Vec<u8> = Vec::new();
    let mut sum = [0f32; 3];
    let mut candidates: Vec<usize> = Vec::new();
    let new_of = |t: usize, local: &[u8]| {
        tris[t]
            .iter()
            .filter(|&&v| local[v as usize] == NONE)
            .count()
    };
    // the meshlet's box, and whether a triangle keeps it within `max_extent`
    let mut bounds = ([f32::MAX; 3], [f32::MIN; 3]);
    let within = |t: usize, (lo, hi): &([f32; 3], [f32; 3])| {
        (0..3).all(|k| {
            let ps = tris[t].map(|v| positions[v as usize][k]);
            let (a, b) = (
                ps.iter().copied().fold(lo[k], f32::min),
                ps.iter().copied().fold(hi[k], f32::max),
            );
            b - a <= max_extent
        })
    };
    for _ in 0..tris.len() {
        let n = local_tris.len() / 3;
        // the best triangle around the meshlet
        let mut best: Option<(usize, usize, f32)> = None;
        if n < MAX_TRIANGLES {
            let mid = sum.map(|x| x / n.max(1) as f32);
            candidates.retain(|&t| !used[t]);
            for &t in &candidates {
                let new = new_of(t, &local);
                if verts.len() + new > MAX_VERTICES || !within(t, &bounds) {
                    continue;
                }
                let c = centres[t];
                let dist = (0..3).map(|k| (c[k] - mid[k]).powi(2)).sum::<f32>();
                if best.is_none_or(|(_, bn, bd)| (new, dist) < (bn, bd)) {
                    best = Some((t, new, dist));
                }
            }
        }
        let t = match best {
            Some((t, _, _)) => t,
            None => {
                // nothing around fits: the next unused triangle in Z-order, in this meshlet if the meshlet has run
                // out of neighbours and has room, else in a new one
                while used[order[cursor]] {
                    cursor += 1;
                }
                let t = order[cursor];
                let room = n < MAX_TRIANGLES
                    && verts.len() + new_of(t, &local) <= MAX_VERTICES
                    && within(t, &bounds);
                if n > 0 && (!room || !candidates.is_empty()) {
                    for &v in &verts {
                        local[v as usize] = NONE;
                    }
                    out.push((std::mem::take(&mut verts), std::mem::take(&mut local_tris)));
                    sum = [0.0; 3];
                    bounds = ([f32::MAX; 3], [f32::MIN; 3]);
                }
                candidates.clear();
                t
            }
        };
        used[t] = true;
        for &v in &tris[t] {
            if local[v as usize] == NONE {
                local[v as usize] = verts.len() as u8;
                verts.push(v);
                candidates.extend(&around[start[v as usize]..start[v as usize + 1]]);
            }
            local_tris.push(local[v as usize]);
            for (k, &x) in positions[v as usize].iter().enumerate() {
                bounds.0[k] = bounds.0[k].min(x);
                bounds.1[k] = bounds.1[k].max(x);
            }
        }
        for k in 0..3 {
            sum[k] += centres[t][k];
        }
    }
    if !local_tris.is_empty() {
        out.push((verts, local_tris));
    }
    out
}

/// How a meshlet writes one UV set: the bases, scale exponents and bits of u and v, and each vertex's offsets.
struct UvForm {
    base: [i16; 2],
    exponent: [u32; 2],
    bits: [u32; 2],
    offsets: [Vec<u32>; 2],
}

/// How a meshlet writes one colour set: the record's mask and constants words, the constant channels, and whether
/// the varying ones are f32.
struct ColourForm {
    mask: u32,
    constants: u32,
    constant: [bool; 4],
    wide: bool,
}

/// A meshlet buffer with `flags` (the shape's: which attributes the vertices have) and position step `step` for
/// `lods`, most detailed first: each LOD's vertices and triangles, with the attributes the flags name (tangents with
/// [`flags::TANGENTS`], as many UV sets, the colour sets `_c0` / `_c1` the flags have, in that order).
pub fn encode(flags: u32, step: f32, lods: &[MeshletGeometry]) -> Result<Encoded> {
    if lods.is_empty() || lods.len() > 16 {
        return Err(invalid(format!("{} LODs (1 to 16)", lods.len())));
    }
    if !(step > 0.0 && step.is_finite()) {
        return Err(invalid(format!("position step {step}")));
    }
    let tangents = flags & flags::TANGENTS != 0;
    let uv_sets = uv_set_count(flags);
    let colour_sets: Vec<usize> = [(0, flags::COLOUR_0), (1, flags::COLOUR_1)]
        .into_iter()
        .filter(|&(_, f)| flags & f != 0)
        .map(|(s, _)| s)
        .collect();
    for (k, g) in lods.iter().enumerate() {
        let n = g.positions.len();
        let fail = |what: String| Err(invalid(format!("LOD {k}: {what}")));
        if g.normals.len() != n {
            return fail("normals not one a vertex".into());
        }
        if tangents != g.tangents.is_some() || g.tangents.as_ref().is_some_and(|t| t.len() != n) {
            return fail("tangents not as the shape has them".into());
        }
        if g.uvs.len() != uv_sets || g.uvs.iter().any(|u| u.len() != n) {
            return fail(format!("{} UV sets, the shape has {uv_sets}", g.uvs.len()));
        }
        if g.colours.iter().map(|c| c.0).collect::<Vec<_>>() != colour_sets
            || g.colours.iter().any(|c| c.1.len() != n)
        {
            return fail("colour sets not as the shape has them".into());
        }
        if g.indices.len() % 3 != 0 || g.indices.iter().any(|&i| i as usize >= n) {
            return fail("indices not triangles of its vertices".into());
        }
        if g.positions.iter().flatten().any(|x| !x.is_finite()) {
            return fail("a position not a number".into());
        }
    }
    let mut bounds_min = [0f32; 3];
    for k in 0..3 {
        bounds_min[k] = lods
            .iter()
            .flat_map(|g| &g.positions)
            .map(|p| p[k])
            .reduce(f32::min)
            .unwrap_or(0.0);
    }
    let normal_cos = NORMAL_ERROR_DEGREES.to_radians().cos();
    let rsize = record_size(flags);

    let groups: Vec<Vec<(Vec<u32>, Vec<u8>)>> = lods
        .iter()
        .map(|g| build_meshlets(&g.positions, &g.indices, ((1u32 << 20) - 4) as f32 * step))
        .collect();
    let lod_meshlets: Vec<u32> = groups.iter().map(|b| b.len() as u32).collect();
    let lod_triangles: Vec<u32> = groups
        .iter()
        .map(|b| b.iter().map(|m| m.1.len() as u32 / 3).sum())
        .collect();
    let total: usize = groups.iter().map(Vec::len).sum();
    let data_start = HEADER_SIZE + total * rsize;
    let mut records: Vec<Vec<u32>> = Vec::with_capacity(total);
    let mut blocks: Vec<u8> = Vec::new();
    let mut written = Vec::new();
    for (g, built) in lods.iter().zip(&groups) {
        for (verts, local_tris) in built {
            let vs: Vec<usize> = verts.iter().map(|&v| v as usize).collect();
            let mut w = vec![0u32; rsize / 4];
            // positions: steps from the minimum, then from the meshlet's base
            let mut q: Vec<[u64; 3]> = Vec::with_capacity(vs.len());
            for &v in &vs {
                let p = g.positions[v];
                let qv: [u64; 3] = std::array::from_fn(|k| {
                    ((p[k] - bounds_min[k]) / step).round().max(0.0) as u64
                });
                if qv.iter().any(|&x| x >= 1 << 21) {
                    return Err(unsupported(format!(
                        "a model over {} m across",
                        ((1u64 << 21) as f32 * step) as u32
                    )));
                }
                written.push(std::array::from_fn(|k| bounds_min[k] + qv[k] as f32 * step));
                q.push(qv);
            }
            let base: [u64; 3] = std::array::from_fn(|k| q.iter().map(|x| x[k]).min().unwrap_or(0));
            let largest: [u64; 3] =
                std::array::from_fn(|k| q.iter().map(|x| x[k] - base[k]).max().unwrap_or(0));
            if largest.iter().any(|&x| 2 * x >= 1 << 21) {
                return Err(unsupported(format!(
                    "a meshlet over {} m across",
                    ((1u64 << 20) as f32 * step) as u32
                )));
            }
            let pbits: [u32; 3] = largest.map(bit_length);
            let big = base[0] as u128
                | (base[1] as u128) << 21
                | (base[2] as u128) << 42
                | ((2 * largest[0]) as u128) << 63
                | ((2 * largest[1]) as u128) << 84
                | ((2 * largest[2]) as u128) << 105;
            for (k, word) in w.iter_mut().take(4).enumerate() {
                *word = (big >> (32 * k)) as u32;
            }
            // normals: the fewest bits that keep each close
            let units: Vec<[f32; 3]> = vs.iter().map(|&v| normalize(g.normals[v])).collect();
            let close_at = |bits: u32| {
                let max = ((1u64 << bits) - 1) as f32;
                units.iter().all(|&n| {
                    let [a, b] = octahedral_encode(n, bits);
                    let d = octahedral(a as f32 / max, b as f32 / max);
                    d[0] * n[0] + d[1] * n[1] + d[2] * n[2] >= normal_cos
                })
            };
            let nbits = (6..=12).find(|&b| close_at(b)).unwrap_or(12);
            // UVs: per component the finest scale whose base fits an i16 and offsets 16 bits
            let mut uv_forms = Vec::with_capacity(g.uvs.len());
            for uv in &g.uvs {
                let mut f = UvForm {
                    base: [0; 2],
                    exponent: [0; 2],
                    bits: [0; 2],
                    offsets: Default::default(),
                };
                for (c, offsets_c) in f.offsets.iter_mut().enumerate() {
                    let fit = (4..=7u32).find_map(|e| {
                        let scale = (2f32).powi(16 - e as i32);
                        let q: Vec<i64> = vs
                            .iter()
                            .map(|&v| (uv[v][c] * scale).round() as i64)
                            .collect();
                        let lo = q.iter().copied().min().unwrap_or(0);
                        let hi = q.iter().copied().max().unwrap_or(0);
                        let fits = vs.iter().all(|&v| uv[v][c].is_finite())
                            && (i16::MIN as i64..=i16::MAX as i64).contains(&lo)
                            && hi - lo < 1 << 16;
                        fits.then(|| {
                            (
                                e,
                                lo,
                                q.iter().map(|x| (x - lo) as u32).collect::<Vec<u32>>(),
                            )
                        })
                    });
                    let Some((e, lo, offsets)) = fit else {
                        return Err(unsupported(
                            "UVs past 64 from 0, or spread over more than 128 in one meshlet",
                        ));
                    };
                    f.base[c] = lo as i16;
                    f.exponent[c] = e;
                    f.bits[c] =
                        bit_length(offsets.iter().copied().max().unwrap_or(0) as u64).max(1);
                    *offsets_c = offsets;
                }
                uv_forms.push(f);
            }
            // colours: constant channels in the record, the rest 8-bit unless one leaves 0..1
            let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
            let mut colour_forms = Vec::with_capacity(g.colours.len());
            for (_, values) in &g.colours {
                let mut f = ColourForm {
                    mask: 0,
                    constants: 0,
                    constant: [false; 4],
                    wide: false,
                };
                for (c, constant) in f.constant.iter_mut().enumerate() {
                    let unit = vs.iter().all(|&v| (0.0..=1.0).contains(&values[v][c]));
                    let first = byte(values[vs[0]][c]);
                    if unit && vs.iter().all(|&v| byte(values[v][c]) == first) {
                        *constant = true;
                        f.mask |= 1 << c;
                        f.constants |= first << (8 * c);
                    }
                    if unit {
                        f.mask |= 1 << (4 + c);
                    } else {
                        f.wide = true;
                    }
                }
                colour_forms.push(f);
            }
            // the record
            let vbits = pbits.iter().sum::<u32>()
                + 2 * nbits
                + if tangents { 17 } else { 0 }
                + colour_forms
                    .iter()
                    .map(|f| {
                        f.constant.iter().filter(|&&c| !c).count() as u32
                            * if f.wide { 32 } else { 8 }
                    })
                    .sum::<u32>()
                + uv_forms.iter().map(|f| f.bits[0] + f.bits[1]).sum::<u32>();
            if vbits >= 1 << 9 {
                return Err(unsupported(format!("{vbits} bits a vertex")));
            }
            let at = data_start + blocks.len();
            w[4] = ((vs.len() as u32) << 24) | ((local_tris.len() as u32 / 3) << 16);
            w[5] = (nbits - 6) | (((at - HEADER_SIZE) as u32) << 4);
            w[6] = vbits | NO_CONE << 9 | pbits[0] << 16 | pbits[1] << 21 | pbits[2] << 26;
            w[7] = if tangents { 2 } else { 0 };
            for (k, f) in uv_forms.iter().enumerate() {
                w[8 + 2 * k] = (f.base[0] as u16 as u32) | (f.base[1] as u16 as u32) << 16;
                w[9 + 2 * k] = f.exponent[0]
                    | f.exponent[1] << 12
                    | (f.bits[0] - 1) << 24
                    | (f.bits[1] - 1) << 28;
            }
            let colour_at = 8 + 2 * uv_forms.len();
            for (k, f) in colour_forms.iter().enumerate() {
                w[colour_at + 2 * k] = f.mask;
                w[colour_at + 2 * k + 1] = f.constants;
            }
            // the vertices, then the indices
            let mut b = BitWriter::default();
            for (i, &v) in vs.iter().enumerate() {
                for k in 0..3 {
                    b.put((q[i][k] - base[k]) as u32, pbits[k]);
                }
                let [nx, ny] = octahedral_encode(units[i], nbits);
                b.put(nx, nbits);
                b.put(ny, nbits);
                if let Some(t) = &g.tangents {
                    let t = t[v];
                    b.put((t[3] >= 0.0) as u32, 1);
                    let [tx, ty] = octahedral_encode([t[0], t[1], t[2]], 8);
                    b.put(tx, 8);
                    b.put(ty, 8);
                }
                for ((_, values), f) in g.colours.iter().zip(&colour_forms) {
                    for (&x, _) in values[v]
                        .iter()
                        .zip(f.constant)
                        .filter(|(_, constant)| !constant)
                    {
                        if f.wide {
                            b.put(x.to_bits(), 32);
                        } else {
                            b.put(byte(x), 8);
                        }
                    }
                }
                for f in &uv_forms {
                    b.put(f.offsets[0][i], f.bits[0]);
                    b.put(f.offsets[1][i], f.bits[1]);
                }
            }
            let ib = index_bits(vs.len());
            for &ix in local_tris {
                b.put(ix as u32, ib);
            }
            blocks.extend_from_slice(&b.bytes);
            records.push(w);
        }
    }
    let mut data = Vec::with_capacity(data_start + blocks.len() + 0x100);
    for x in bounds_min.iter().chain([&step]) {
        data.extend(x.to_le_bytes());
    }
    data.extend(flags.to_le_bytes());
    let mut running = 0u32;
    let totals: Vec<u32> = lod_meshlets
        .iter()
        .map(|&n| {
            running += n;
            running
        })
        .collect();
    for k in 0..16 {
        data.extend(totals[k.min(totals.len() - 1)].to_le_bytes());
    }
    for w in &records {
        for x in w {
            data.extend(x.to_le_bytes());
        }
    }
    data.extend_from_slice(&blocks);
    data.resize(data.len().next_multiple_of(0x100), 0);
    Ok(Encoded {
        data,
        bounds_min,
        lod_meshlets,
        lod_triangles,
        positions: written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Each triangle's corners (positions in steps), its smallest corner first, the winding kept; sorted.
    fn corner_triangles(g: &MeshletGeometry) -> Vec<[[i64; 3]; 3]> {
        let q = |v: u32| g.positions[v as usize].map(|x| (x / STEP).round() as i64);
        let mut out: Vec<[[i64; 3]; 3]> = g
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let c = [q(t[0]), q(t[1]), q(t[2])];
                let first = (0..3).min_by_key(|&k| c[k]).unwrap();
                [c[first], c[(first + 1) % 3], c[(first + 2) % 3]]
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn encoded_buffers_decode_to_what_was_given() {
        use crate::testing::{MESHLET_GRID_FLAGS, meshlet_grid};
        // two LODs: a grid of 288 triangles (several meshlets), UV set 1 tiling 20 times (a coarser scale), and one
        // of 8 whose colours leave 0..1 (f32 colours)
        let lods = [
            meshlet_grid(12, 3.0, [-1.0, 0.25, 2.0], 20.0, 1.0),
            meshlet_grid(2, 3.0, [-1.0, 0.25, 2.0], 20.0, 4.0),
        ];
        let e = encode(MESHLET_GRID_FLAGS, STEP, &lods).unwrap();
        assert_eq!(e.data.len() % 0x100, 0);
        assert_eq!(e.lod_triangles, vec![288, 8]);
        assert!(e.lod_meshlets[0] >= 3, "{:?}", e.lod_meshlets);
        assert_eq!(e.bounds_min[0], -1.0);
        let s = MeshletShape::parse(&e.data, 0, 2).unwrap();
        assert_eq!(s.flags, MESHLET_GRID_FLAGS);
        for (lod, given) in lods.iter().enumerate() {
            let range = s.lods[lod].clone();
            assert_eq!(range.len(), e.lod_meshlets[lod] as usize);
            for m in &s.meshlets[range] {
                assert!(m.vertex_count <= MAX_VERTICES && m.triangle_count <= MAX_TRIANGLES);
                assert_eq!(m.record[7], 2, "tangents");
                assert_eq!((m.record[6] >> 9) & 0x7f, NO_CONE);
            }
            let g = s.geometry(lod).unwrap();
            // the same triangles; every corner's values as given, within what was written
            assert_eq!(corner_triangles(&g), corner_triangles(given));
            let w = g.welded();
            for (v, p) in w.positions.iter().enumerate() {
                let i = given
                    .positions
                    .iter()
                    .position(|q| (0..3).all(|k| (p[k] - q[k]).abs() <= STEP / 2.0 + 1e-6))
                    .expect("a vertex where one was given");
                let n = w.normals[v];
                assert!(n[1] > (0.5f32).to_radians().cos(), "{n:?}");
                let t = w.tangents.as_ref().unwrap()[v];
                assert!(
                    t[0] > 0.999 && t[3] == given.tangents.as_ref().unwrap()[i][3],
                    "{t:?}"
                );
                for set in 0..2 {
                    for c in 0..2 {
                        let d = (w.uvs[set][v][c] - given.uvs[set][i][c]).abs();
                        assert!(d <= (2f32).powi(-10), "uv {set}: {d}");
                    }
                }
                let (given_c, ours) = (given.colours[0].1[i], w.colours[0].1[v]);
                assert_eq!(ours[0], 1.0, "constant");
                for c in [1, 3] {
                    if lod == 1 {
                        assert_eq!(ours[c], given_c[c], "f32 colours are exact");
                    } else {
                        assert!((ours[c] - given_c[c]).abs() <= 0.5 / 255.0 + 1e-6);
                    }
                }
            }
        }
        // the record's colour mask: channel 0 constant (0xff), all four in 0..1 in LOD 0 (8-bit); in LOD 1 channel
        // 3 isn't (f32 for the varying ones)
        let colour_word = |m: &Meshlet| m.record[8 + 2 * 2];
        assert_eq!(colour_word(&s.meshlets[0]) & 0xff, 0xf0 | 1 | 4);
        let last = s.meshlets.last().unwrap();
        assert_eq!(colour_word(last) & 0xff, 0x70 | 1 | 4);
        assert_eq!(last.colour_sets[0].channels[3], Channel::Float);
    }

    #[test]
    fn meshlets_keep_to_their_limits() {
        let g = crate::testing::meshlet_grid(40, 10.0, [0.0; 3], 1.0, 1.0);
        let mut indices = g.indices.clone();
        indices.extend([0, 0, 1]); // a triangle that uses a vertex twice: left out
        let built = build_meshlets(&g.positions, &indices, 1000.0);
        let mut seen = std::collections::BTreeSet::new();
        for (verts, tris) in &built {
            assert!(verts.len() <= MAX_VERTICES && tris.len() / 3 <= MAX_TRIANGLES);
            // vertices in the order the triangles first use them
            let mut next = 0u8;
            for &i in tris {
                assert!(i <= next);
                if i == next {
                    next += 1;
                }
            }
            for t in tris.as_chunks::<3>().0 {
                let mut key = t.iter().map(|&i| verts[i as usize]).collect::<Vec<_>>();
                key.sort();
                assert!(seen.insert(key), "a triangle twice");
            }
        }
        assert_eq!(seen.len(), 3200);
        // a grid's meshlets come out near full (64 vertices hold about 98 of its triangles)
        assert!(built.len() <= 3200 / 80, "{} meshlets", built.len());
    }

    #[test]
    fn a_meshlet_stays_within_its_extent() {
        // two triangles 10 apart: one meshlet would do, but not within 5
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [10.0, 0.0, 0.0],
            [11.0, 0.0, 0.0],
            [10.0, 1.0, 0.0],
        ];
        let indices = [0, 1, 2, 3, 4, 5];
        assert_eq!(build_meshlets(&positions, &indices, 20.0).len(), 1);
        assert_eq!(build_meshlets(&positions, &indices, 5.0).len(), 2);
    }

    #[test]
    fn octahedral_encoding_is_close() {
        let mut worst = 0f32;
        for i in 0..200 {
            let (a, b) = (i as f32 * 0.37, i as f32 * 0.11);
            let n = [a.cos() * b.sin(), a.sin() * b.sin(), b.cos()];
            for bits in [6, 8, 12] {
                let [x, y] = octahedral_encode(n, bits);
                let max = ((1u64 << bits) - 1) as f32;
                let d = octahedral(x as f32 / max, y as f32 / max);
                let angle = (d[0] * n[0] + d[1] * n[1] + d[2] * n[2])
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees();
                if bits == 8 {
                    worst = worst.max(angle);
                }
                assert!(angle < 3.0, "{bits} bits: {angle}");
            }
        }
        assert!(worst < 0.8, "8 bits: {worst}");
    }

    #[test]
    fn a_model_too_large_is_refused() {
        let mut g = crate::testing::meshlet_grid(1, 1.0, [0.0; 3], 1.0, 1.0);
        g.positions[3] = [2000.0, 0.0, 0.0];
        let e = encode(crate::testing::MESHLET_GRID_FLAGS, STEP, &[g])
            .unwrap_err()
            .to_string();
        assert!(e.contains("across"), "{e}");
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
