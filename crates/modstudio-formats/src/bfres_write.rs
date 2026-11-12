//! Writing a BFRES file: the original file with new geometry for some of its shapes.
//!
//! The file keeps everything the game reads before its GPU data where it is (structures, names, materials,
//! skeleton); only fields are patched: vertex counts, buffer offsets and sizes, index counts and formats, sub-mesh
//! ranges, bounding boxes and spheres. The GPU data (index buffers, then vertex buffers) is laid out again the way
//! the game's files are, and what follows it (the memory pool block and the relocation table) moves with its end;
//! the relocation table's sections and the pointers into the moved part are adjusted. A file written without
//! replacements is byte-identical to the original.
//!
//! Layout and bounds rules were taken from the game's files (checked against every model): index buffers in shape
//! and LOD order, each on 256 bytes; vertex buffers in order, each buffer on its vertex buffer's alignment; the GPU
//! block padded to 4 KiB. Bounding boxes: per LOD, each sub-mesh then the whole mesh, around every vertex in the
//! local space of each bone it is weighted to. Spheres: per skin bone, the smallest sphere around the vertices
//! weighted to it (all LODs, in its local space), radius + 0.0001.

use std::collections::{BTreeMap, BTreeSet};

use crate::bfres::{AttribFormat, Bfres, Bounding, Model, Shape};
use crate::bytes::ReadLe;
use crate::error::{Result, invalid, unsupported};

const INDEX_ALIGN: usize = 0x100;
const GPU_END_ALIGN: usize = 0x1000;
const SPHERE_MARGIN: f32 = 0.0001;

/// One LOD (mesh) of new geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct LodGeometry {
    /// Where this LOD's vertices start in the shape's vertex buffer.
    pub first_vertex: u32,
    /// Triangle indices, relative to `first_vertex`.
    pub indices: Vec<u32>,
    /// Indices per sub-mesh, in order; they add up to `indices.len()`. As many as the game's mesh has.
    pub sub_mesh_counts: Vec<u32>,
}

/// New geometry for one shape: its whole vertex buffer and every LOD.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeGeometry {
    pub vertex_count: u32,
    /// One value per vertex for each of the vertex buffer's attributes (stored in the game's formats).
    pub attributes: BTreeMap<String, Vec<[f32; 4]>>,
    /// As many as the shape has meshes.
    pub lods: Vec<LodGeometry>,
}

impl ShapeGeometry {
    /// The shape's own geometry, decoded (for checks: writing it back must give the same bytes).
    pub fn from_shape(model: &Model, shape: &Shape) -> Result<Self> {
        let vb = &model.vertex_buffers[shape.vertex_buffer];
        let mut attributes = BTreeMap::new();
        for a in &vb.attributes {
            attributes.insert(a.name.clone(), vb.read(a)?);
        }
        let lods = shape
            .meshes
            .iter()
            .map(|m| {
                let first = m.first_vertex;
                let indices = m.indices()?.into_iter().map(|i| i - first).collect();
                let sub_mesh_counts = m.sub_meshes.iter().map(|&(_, n)| n).collect();
                Ok(LodGeometry {
                    first_vertex: first,
                    indices,
                    sub_mesh_counts,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ShapeGeometry {
            vertex_count: vb.vertex_count,
            attributes,
            lods,
        })
    }
}

/// The relocation table: sections of the file, and where its pointers are.
#[derive(Debug, Clone)]
struct Relocations {
    at: usize,
    sections: Vec<Section>,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Copy)]
struct Section {
    offset: usize,
    size: usize,
    first_entry: usize,
    entry_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    at: usize,
    structs: usize,
    offsets: usize,
    padding: usize,
}

impl Entry {
    /// Positions of the pointers this entry lists.
    fn pointers(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.structs).flat_map(move |s| {
            (0..self.offsets).map(move |o| self.at + (s * (self.offsets + self.padding) + o) * 8)
        })
    }
}

impl Relocations {
    fn parse(d: &[u8]) -> Result<Self> {
        let at = d.u32_at(0x18)? as usize;
        crate::bytes::check_magic(d, at, "_RLT")?;
        let count = d.u32_at(at + 8)? as usize;
        let sections = (0..count)
            .map(|i| {
                let s = at + 0x10 + i * 0x18;
                Ok(Section {
                    offset: d.u32_at(s + 8)? as usize,
                    size: d.u32_at(s + 12)? as usize,
                    first_entry: d.u32_at(s + 16)? as usize,
                    entry_count: d.u32_at(s + 20)? as usize,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let total: usize = sections.iter().map(|s| s.entry_count).sum();
        let entries_at = at + 0x10 + count * 0x18;
        let entries = (0..total)
            .map(|i| {
                let e = entries_at + i * 8;
                Ok(Entry {
                    at: d.u32_at(e)? as usize,
                    structs: d.u16_at(e + 4)? as usize,
                    offsets: d.u8_at(e + 6)? as usize,
                    padding: d.u8_at(e + 7)? as usize,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Relocations {
            at,
            sections,
            entries,
        })
    }

    fn entries_of(&self, section: usize) -> &[Entry] {
        let s = &self.sections[section];
        &self.entries[s.first_entry..s.first_entry + s.entry_count]
    }
}

/// Writes `bfres` (its first and only model) with `replace`'s shapes (by index) given new geometry.
pub fn write(bfres: &Bfres, replace: &BTreeMap<usize, ShapeGeometry>) -> Result<Vec<u8>> {
    let d = bfres.data;
    let [model] = bfres.models.as_slice() else {
        return Err(unsupported(format!(
            "{} models in one file",
            bfres.models.len()
        )));
    };
    if model.shapes.is_empty() {
        return Err(unsupported("files without meshes (skeletons)"));
    }
    if model.shapes.iter().any(|s| s.is_meshlet()) {
        return Err(unsupported("models with meshlet shapes"));
    }
    let rlt = Relocations::parse(d)?;
    if rlt.sections.len() < 3 {
        return Err(unsupported(format!(
            "{} relocation sections",
            rlt.sections.len()
        )));
    }
    let gpu = bfres.buffer_offset;
    let (index_section, vertex_section) = (rlt.sections[1], rlt.sections[2]);
    if index_section.offset != gpu || vertex_section.offset != gpu + index_section.size {
        return Err(unsupported("GPU data not where the relocation table says"));
    }
    let old_gpu_end = vertex_section.offset + vertex_section.size;
    let buffer_info = d.offset_at(0xB0)?;

    // Which shape owns each vertex buffer (a replaced shape's buffer must be its own).
    for (&i, g) in replace {
        let shape = model
            .shapes
            .get(i)
            .ok_or_else(|| invalid(format!("no shape {i}")))?;
        check_geometry(model, shape, g)?;
        if model
            .shapes
            .iter()
            .enumerate()
            .any(|(j, s)| j != i && s.vertex_buffer == shape.vertex_buffer)
        {
            return Err(unsupported(format!(
                "shape {}: its vertex buffer is shared",
                shape.name
            )));
        }
    }
    let replaced_vb: BTreeMap<usize, &ShapeGeometry> = replace
        .iter()
        .map(|(&i, g)| (model.shapes[i].vertex_buffer, g))
        .collect();

    // The CPU part, patched below; the GPU block, laid out again.
    let mut out = d[..gpu].to_vec();
    let mut block: Vec<u8> = Vec::new();
    let place = |block: &mut Vec<u8>, bytes: &[u8], align: usize| -> usize {
        block.resize((gpu + block.len()).next_multiple_of(align.max(1)) - gpu, 0);
        let at = block.len();
        block.extend_from_slice(bytes);
        at
    };

    // Index buffers, shape by shape, LOD by LOD.
    for (si, shape) in model.shapes.iter().enumerate() {
        for (li, mesh) in shape.meshes.iter().enumerate() {
            let (format, bytes, count, first, ranges) = match replace.get(&si) {
                None => {
                    let size = d.u32_at(d.offset_at(mesh.at + 0x18)?)? as usize;
                    (
                        mesh.index_format,
                        mesh.index_data[..size].to_vec(),
                        mesh.index_count,
                        mesh.first_vertex,
                        None,
                    )
                }
                Some(g) => {
                    let lod = &g.lods[li];
                    let (format, bytes) = encode_indices(&lod.indices, mesh.index_format);
                    let size = index_size(format);
                    let mut ranges = Vec::new();
                    let mut start = 0u32;
                    for &n in &lod.sub_mesh_counts {
                        ranges.push((start * size, n));
                        start += n;
                    }
                    (
                        format,
                        bytes,
                        lod.indices.len() as u32,
                        lod.first_vertex,
                        Some(ranges),
                    )
                }
            };
            let at = place(&mut block, &bytes, INDEX_ALIGN);
            put_u32(&mut out, mesh.at + 0x20, at as u32);
            if let Some(ranges) = ranges {
                put_u32(&mut out, mesh.at + 0x28, format);
                put_u32(&mut out, mesh.at + 0x2C, count);
                put_u32(&mut out, mesh.at + 0x30, first);
                put_u32(&mut out, d.offset_at(mesh.at + 0x18)?, bytes.len() as u32);
                let subs = d.offset_at(mesh.at)?;
                for (k, (offset, n)) in ranges.into_iter().enumerate() {
                    put_u32(&mut out, subs + k * 8, offset);
                    put_u32(&mut out, subs + k * 8 + 4, n);
                }
            }
        }
    }
    let index_end = (gpu + block.len()).next_multiple_of(INDEX_ALIGN) - gpu;
    block.resize(index_end, 0);

    // Vertex buffers, in order.
    for (vi, vb) in model.vertex_buffers.iter().enumerate() {
        let align = d.u16_at(vb.at + 0x56)? as usize;
        let size_array = d.offset_at(vb.at + 0x30)?;
        let new_data = replaced_vb
            .get(&vi)
            .map(|g| encode_vertices(vb, g))
            .transpose()?;
        for (bi, buffer) in vb.buffers.iter().enumerate() {
            let bytes = match &new_data {
                Some(buffers) => &buffers[bi][..],
                None => buffer.data,
            };
            let at = place(&mut block, bytes, align);
            if bi == 0 {
                put_u32(&mut out, vb.at + 0x48, at as u32);
            }
            put_u32(&mut out, size_array + bi * 16, bytes.len() as u32);
        }
        if let Some(g) = replaced_vb.get(&vi) {
            put_u32(&mut out, vb.at + 0x50, g.vertex_count);
        }
    }
    let gpu_end = (gpu + block.len()).next_multiple_of(GPU_END_ALIGN);
    block.resize(gpu_end - gpu, 0);

    // Bounds of the new shapes.
    for (&i, g) in replace {
        let shape = &model.shapes[i];
        let (boxes, spheres) = shape_bounds(model, shape, g)?;
        // Only where the file has room for them (the arrays the reader found).
        if shape.bounding_at != 0 {
            for (k, b) in boxes.iter().enumerate().take(shape.boundings.len()) {
                put_f32s(&mut out, shape.bounding_at + k * 24, &b.center);
                put_f32s(&mut out, shape.bounding_at + k * 24 + 12, &b.extent);
            }
        }
        if shape.radius_at != 0 {
            for (k, s) in spheres.iter().enumerate().take(shape.spheres.len()) {
                put_f32s(&mut out, shape.radius_at + k * 16, s);
            }
        }
    }

    // GPU block, then everything after it, moved by `delta`.
    let delta = gpu_end as i64 - old_gpu_end as i64;
    let shift = |v: usize| (v as i64 + delta) as usize;
    out.extend_from_slice(&block);
    out.extend_from_slice(&d[old_gpu_end..]);

    // Pointers into the moved part, wherever they are, and pointers stored in the moved part.
    for (si, section) in rlt.sections.iter().enumerate() {
        let target_moves = section.offset >= old_gpu_end;
        for entry in rlt.entries_of(si) {
            for at in entry.pointers() {
                let at = if at >= old_gpu_end { shift(at) } else { at };
                if target_moves {
                    let v = out.as_slice().u64_at(at)?;
                    if v != 0 {
                        put_u64(&mut out, at, (v as i64 + delta) as u64);
                    }
                }
            }
        }
    }

    // The relocation table itself.
    let r = shift(rlt.at);
    put_u32(&mut out, 0x18, r as u32);
    put_u32(&mut out, r + 4, r as u32);
    for (si, section) in rlt.sections.iter().enumerate() {
        let s = r + 0x10 + si * 0x18;
        match si {
            1 => put_u32(&mut out, s + 12, index_end as u32),
            2 => {
                put_u32(&mut out, s + 8, (gpu + index_end) as u32);
                put_u32(&mut out, s + 12, (gpu_end - gpu - index_end) as u32);
            }
            _ if section.offset >= old_gpu_end => {
                put_u32(&mut out, s + 8, shift(section.offset) as u32)
            }
            _ => {}
        }
    }
    let entries_at = r + 0x10 + rlt.sections.len() * 0x18;
    for (k, e) in rlt.entries.iter().enumerate() {
        if e.at >= old_gpu_end {
            put_u32(&mut out, entries_at + k * 8, shift(e.at) as u32);
        }
    }

    // File size and the GPU buffer size.
    let len = out.len() as u32;
    put_u32(&mut out, 0x1C, len);
    if buffer_info != 0 {
        put_u32(&mut out, buffer_info + 4, (gpu_end - gpu) as u32);
    }
    Ok(out)
}

fn check_geometry(model: &Model, shape: &Shape, g: &ShapeGeometry) -> Result<()> {
    let name = &shape.name;
    let vb = &model.vertex_buffers[shape.vertex_buffer];
    if g.lods.len() != shape.meshes.len() {
        return Err(invalid(format!(
            "{name}: {} LODs, the game's has {}",
            g.lods.len(),
            shape.meshes.len()
        )));
    }
    let wanted: BTreeSet<&str> = vb.attributes.iter().map(|a| a.name.as_str()).collect();
    let given: BTreeSet<&str> = g.attributes.keys().map(String::as_str).collect();
    if wanted != given {
        return Err(invalid(format!(
            "{name}: attributes {given:?}, the game's are {wanted:?}"
        )));
    }
    if let Some((a, v)) = g
        .attributes
        .iter()
        .find(|(_, v)| v.len() != g.vertex_count as usize)
    {
        return Err(invalid(format!(
            "{name}: {a} has {} values for {} vertices",
            v.len(),
            g.vertex_count
        )));
    }
    for (l, (lod, mesh)) in g.lods.iter().zip(&shape.meshes).enumerate() {
        if lod.sub_mesh_counts.len() != mesh.sub_meshes.len() {
            return Err(invalid(format!(
                "{name} LOD {l}: sub-mesh count differs from the game's"
            )));
        }
        if lod
            .sub_mesh_counts
            .iter()
            .map(|&n| n as usize)
            .sum::<usize>()
            != lod.indices.len()
            || lod.indices.len() % 3 != 0
        {
            return Err(invalid(format!(
                "{name} LOD {l}: sub-meshes don't cover the triangles"
            )));
        }
        if lod
            .indices
            .iter()
            .any(|&i| lod.first_vertex as u64 + i as u64 >= g.vertex_count as u64)
        {
            return Err(invalid(format!("{name} LOD {l}: index past the vertices")));
        }
    }
    Ok(())
}

fn index_size(format: u32) -> u32 {
    match format {
        0 => 1,
        1 => 2,
        _ => 4,
    }
}

/// Indices in the game's format when they fit, else 32-bit.
fn encode_indices(indices: &[u32], format: u32) -> (u32, Vec<u8>) {
    let max = indices.iter().copied().max().unwrap_or(0);
    let format = match format {
        0 if max <= 0xFF => 0,
        0 | 1 if max <= 0xFFFF => 1,
        _ => 2,
    };
    let bytes = match format {
        0 => indices.iter().map(|&i| i as u8).collect(),
        1 => indices
            .iter()
            .flat_map(|&i| (i as u16).to_le_bytes())
            .collect(),
        _ => indices.iter().flat_map(|&i| i.to_le_bytes()).collect(),
    };
    (format, bytes)
}

/// The vertex buffer's buffers with `g`'s values, in the game's formats and strides.
fn encode_vertices(vb: &crate::bfres::VertexBuffer, g: &ShapeGeometry) -> Result<Vec<Vec<u8>>> {
    let mut buffers: Vec<Vec<u8>> = vb
        .buffers
        .iter()
        .map(|b| vec![0u8; b.stride as usize * g.vertex_count as usize])
        .collect();
    for a in &vb.attributes {
        let values = &g.attributes[&a.name];
        let size = a.format.size().ok_or_else(|| {
            unsupported(format!("attribute {}: format {}", a.name, a.format.name()))
        })?;
        let stride = vb.buffers[a.buffer as usize].stride as usize;
        let out = &mut buffers[a.buffer as usize];
        for (v, value) in values.iter().enumerate() {
            let at = v * stride + a.offset as usize;
            a.format
                .encode(*value, &mut out[at..at + size])
                .ok_or_else(|| {
                    unsupported(format!("attribute {}: format {}", a.name, a.format.name()))
                })?;
        }
    }
    Ok(buffers)
}

impl AttribFormat {
    /// Writes one vertex's value (the inverse of `decode`, rounding to the nearest step).
    pub fn encode(self, v: [f32; 4], out: &mut [u8]) -> Option<()> {
        let size = self.size()?;
        let out = out.get_mut(..size)?;
        let kind = self.kind();
        let channels = self.channels();
        if channels == 0x0E {
            let unorm = |x: f32, max: f32| (x.clamp(0.0, 1.0) * max).round() as u32;
            let snorm = |x: f32| ((x.clamp(-1.0, 1.0) * 511.0).round() as i32 as u32) & 0x3FF;
            let word = match kind {
                1 => {
                    unorm(v[0], 1023.0)
                        | unorm(v[1], 1023.0) << 10
                        | unorm(v[2], 1023.0) << 20
                        | unorm(v[3], 3.0) << 30
                }
                2 => {
                    let w = (v[3].round().clamp(-2.0, 1.0) as i32 as u32) & 3;
                    snorm(v[0]) | snorm(v[1]) << 10 | snorm(v[2]) << 20 | w << 30
                }
                3 | 8 => {
                    let u = |x: f32, max: f32| x.round().clamp(0.0, max) as u32;
                    u(v[0], 1023.0)
                        | u(v[1], 1023.0) << 10
                        | u(v[2], 1023.0) << 20
                        | u(v[3], 3.0) << 30
                }
                _ => return None,
            };
            out.copy_from_slice(&word.to_le_bytes());
            return Some(());
        }
        let n = self.components()?;
        let bytes = size / n;
        for (c, &x) in v.iter().enumerate().take(n) {
            let slot = &mut out[c * bytes..(c + 1) * bytes];
            match (bytes, kind) {
                (1, 1) => slot[0] = (x.clamp(0.0, 1.0) * 255.0).round() as u8,
                (1, 2) => slot[0] = (x.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8,
                (1, 3 | 8) => slot[0] = x.round().clamp(0.0, 255.0) as u8,
                (1, 4 | 9) => slot[0] = x.round().clamp(-128.0, 127.0) as i8 as u8,
                (2, _) => {
                    let u: u16 = match kind {
                        1 => (x.clamp(0.0, 1.0) * 65535.0).round() as u16,
                        2 => (x.clamp(-1.0, 1.0) * 32767.0).round() as i16 as u16,
                        3 | 8 => x.round().clamp(0.0, 65535.0) as u16,
                        4 | 9 => x.round().clamp(-32768.0, 32767.0) as i16 as u16,
                        5 => f32_to_half(x),
                        _ => return None,
                    };
                    slot.copy_from_slice(&u.to_le_bytes());
                }
                (4, _) => {
                    let u: u32 = match kind {
                        3 | 8 => x.round().max(0.0) as u32,
                        4 | 9 => x.round() as i32 as u32,
                        5 => x.to_bits(),
                        _ => return None,
                    };
                    slot.copy_from_slice(&u.to_le_bytes());
                }
                _ => return None,
            }
        }
        Some(())
    }
}

/// IEEE 754 single to half precision, rounding to nearest even.
pub fn f32_to_half(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mant = bits & 0x7F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1F {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rest = m & ((1 << shift) - 1);
        let mut h = m >> shift;
        if rest > half || (rest == half && h & 1 == 1) {
            h += 1;
        }
        return sign | h as u16;
    }
    let mut h = ((e as u32) << 10) | (mant >> 13);
    let rest = mant & 0x1FFF;
    if rest > 0x1000 || (rest == 0x1000 && h & 1 == 1) {
        h += 1;
    }
    sign | h as u16
}

// ---------------------------------------------------------------- bounds

/// (vertex, bone, position in the bone's space, whether the skin slot has weight)
type LocalPoint = (u32, usize, [f32; 3], bool);

/// Every vertex's position in the local space of each bone of its skin slots: (vertex, bone, position, whether
/// the slot has weight). Empty slots count too: the game's boxes include them (their bone is matrix 0's).
fn local_positions(model: &Model, shape: &Shape, g: &ShapeGeometry) -> Result<Vec<LocalPoint>> {
    let pos = g
        .attributes
        .get("_p0")
        .ok_or_else(|| invalid(format!("{}: no _p0", shape.name)))?;
    let skin = shape.skin_count as usize;
    let mut out = Vec::with_capacity(pos.len());
    if skin <= 1 {
        // Rigid and unskinned vertices are stored in their bone's space already.
        let i0 = g.attributes.get("_i0");
        for (v, p) in pos.iter().enumerate() {
            let bone = match (skin, i0) {
                (1, Some(i0)) => matrix_bone(model, i0[v][0])?,
                _ => shape.bone,
            };
            out.push((v as u32, bone, [p[0], p[1], p[2]], true));
        }
        return Ok(out);
    }
    let sets = [
        (g.attributes.get("_i0"), g.attributes.get("_w0")),
        (g.attributes.get("_i1"), g.attributes.get("_w1")),
    ];
    for (v, p) in pos.iter().enumerate() {
        for k in 0..skin {
            let (Some(i), Some(w)) = sets[k / 4] else {
                continue;
            };
            let weighted = w[v][k % 4] > 0.0;
            let bone = matrix_bone(model, i[v][k % 4])?;
            let m = model.skeleton.bones[bone]
                .smooth_matrix
                .and_then(|s| model.skeleton.inverse_model_matrices.get(s))
                .ok_or_else(|| {
                    invalid(format!("{}: bone {bone} has no smooth matrix", shape.name))
                })?;
            let local = [0, 1, 2].map(|r| {
                m[r * 4] * p[0] + m[r * 4 + 1] * p[1] + m[r * 4 + 2] * p[2] + m[r * 4 + 3]
            });
            out.push((v as u32, bone, local, weighted));
        }
    }
    Ok(out)
}

fn matrix_bone(model: &Model, index: f32) -> Result<usize> {
    model
        .skeleton
        .matrix_to_bone
        .get(index as usize)
        .map(|&b| b as usize)
        .ok_or_else(|| invalid(format!("bone matrix {index} out of range")))
}

fn bounding(points: impl Iterator<Item = [f32; 3]>) -> Bounding {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for c in 0..3 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
        }
    }
    if lo[0] > hi[0] {
        return Bounding {
            center: [0.0; 3],
            extent: [0.0; 3],
        };
    }
    Bounding {
        center: [0, 1, 2].map(|c| (lo[c] + hi[c]) / 2.0),
        extent: [0, 1, 2].map(|c| (hi[c] - lo[c]) / 2.0),
    }
}

/// The shape's bounding boxes (per LOD: each sub-mesh, then the mesh) and spheres (per skin bone, or per LOD
/// without skin bones), computed the game's way.
pub fn shape_bounds(
    model: &Model,
    shape: &Shape,
    g: &ShapeGeometry,
) -> Result<(Vec<Bounding>, Vec<[f32; 4]>)> {
    let local = local_positions(model, shape, g)?;
    let mut by_vertex: Vec<Vec<[f32; 3]>> = vec![Vec::new(); g.vertex_count as usize];
    for &(v, _, p, _) in &local {
        by_vertex[v as usize].push(p);
    }
    let mut boxes = Vec::new();
    for lod in &g.lods {
        let mut start = 0usize;
        for &n in &lod.sub_mesh_counts {
            let range = &lod.indices[start..start + n as usize];
            let verts: BTreeSet<u32> = range.iter().map(|&i| i + lod.first_vertex).collect();
            boxes.push(bounding(
                verts
                    .iter()
                    .flat_map(|&v| by_vertex[v as usize].iter().copied()),
            ));
            start += n as usize;
        }
        let verts: BTreeSet<u32> = lod.indices.iter().map(|&i| i + lod.first_vertex).collect();
        let points: Vec<[f32; 3]> = verts
            .iter()
            .flat_map(|&v| by_vertex[v as usize].iter().copied())
            .collect();
        boxes.push(bounding(points.iter().copied()));
    }
    let spheres = if shape.skin_bone_indices.is_empty() {
        // One sphere for the shape, around all of its vertices (in its bone's space).
        let all: Vec<[f32; 3]> = local.iter().map(|&(_, _, p, _)| p).collect();
        vec![smallest_sphere(&all)]
    } else {
        shape
            .skin_bone_indices
            .iter()
            .map(|&b| {
                let points: Vec<[f32; 3]> = local
                    .iter()
                    .filter(|&&(_, bone, _, weighted)| weighted && bone == b as usize)
                    .map(|&(_, _, p, _)| p)
                    .collect();
                smallest_sphere(&points)
            })
            .collect()
    };
    Ok((boxes, spheres))
}

type V = [f64; 3];

fn vsub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn vdot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vcross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// (centre, radius squared)
type Ball = (V, f64);

fn holds(s: &Ball, q: V) -> bool {
    vdot(vsub(q, s.0), vsub(q, s.0)) <= s.1 * (1.0 + 1e-10) + 1e-12
}

/// The sphere with all of `b` (1 to 4 points) on its surface, if there is exactly one.
fn exact_sphere(b: &[V]) -> Option<Ball> {
    match b.len() {
        1 => Some((b[0], 0.0)),
        2 => {
            let c = [0, 1, 2].map(|i| (b[0][i] + b[1][i]) / 2.0);
            Some((c, vdot(vsub(b[0], c), vsub(b[0], c))))
        }
        3 => {
            let (a, bb) = (vsub(b[1], b[0]), vsub(b[2], b[0]));
            let n = vcross(a, bb);
            let den = 2.0 * vdot(n, n);
            if den.abs() < 1e-24 {
                return None;
            }
            let t = [0, 1, 2]
                .map(|i| (vdot(a, a) * vcross(bb, n)[i] + vdot(bb, bb) * vcross(n, a)[i]) / den);
            Some(([0, 1, 2].map(|i| b[0][i] + t[i]), vdot(t, t)))
        }
        _ => {
            let (a, bb, cc) = (vsub(b[1], b[0]), vsub(b[2], b[0]), vsub(b[3], b[0]));
            let den = 2.0 * vdot(a, vcross(bb, cc));
            if den.abs() < 1e-24 {
                return None;
            }
            let t = [0, 1, 2].map(|i| {
                (vdot(a, a) * vcross(bb, cc)[i]
                    + vdot(bb, bb) * vcross(cc, a)[i]
                    + vdot(cc, cc) * vcross(a, bb)[i])
                    / den
            });
            Some(([0, 1, 2].map(|i| b[0][i] + t[i]), vdot(t, t)))
        }
    }
}

/// The sphere through `b`; when the points are collinear or coplanar (no single such sphere), the smallest
/// sphere through some of them that holds them all.
fn sphere_through(b: &[V]) -> Ball {
    if let Some(s) = exact_sphere(b) {
        return s;
    }
    let mut best: Option<Ball> = None;
    for skip in 0..b.len() {
        let rest: Vec<V> = b
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != skip)
            .map(|(_, &v)| v)
            .collect();
        let s = sphere_through(&rest);
        if b.iter().all(|&q| holds(&s, q)) && best.is_none_or(|x| s.1 < x.1) {
            best = Some(s);
        }
    }
    best.expect("two of the points always make a sphere that holds collinear ones")
}

/// The smallest sphere around `points` (Welzl's algorithm, iterative), radius + the game's margin.
pub fn smallest_sphere(points: &[[f32; 3]]) -> [f32; 4] {
    let p: Vec<V> = points.iter().map(|q| q.map(f64::from)).collect();
    if p.is_empty() {
        return [0.0, 0.0, 0.0, SPHERE_MARGIN];
    }
    let mut s = sphere_through(&[p[0]]);
    for i in 1..p.len() {
        if holds(&s, p[i]) {
            continue;
        }
        s = sphere_through(&[p[i]]);
        for j in 0..i {
            if holds(&s, p[j]) {
                continue;
            }
            s = sphere_through(&[p[i], p[j]]);
            for k in 0..j {
                if holds(&s, p[k]) {
                    continue;
                }
                s = sphere_through(&[p[i], p[j], p[k]]);
                for l in 0..k {
                    if holds(&s, p[l]) {
                        continue;
                    }
                    s = sphere_through(&[p[i], p[j], p[k], p[l]]);
                }
            }
        }
    }
    let r = s.1.max(0.0).sqrt() as f32 + SPHERE_MARGIN;
    [s.0[0] as f32, s.0[1] as f32, s.0[2] as f32, r]
}

fn put_u32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut [u8], at: usize, v: u64) {
    out[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn put_f32s(out: &mut [u8], at: usize, v: &[f32]) {
    for (k, x) in v.iter().enumerate() {
        out[at + k * 4..at + k * 4 + 4].copy_from_slice(&x.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bfres::half_to_f32;

    use crate::bfres::Bfres;
    use crate::testing::tiny_bfres;

    #[test]
    fn unchanged_model_writes_back_identically() {
        let file = tiny_bfres();
        let bfres = Bfres::parse(&file, None).unwrap();
        assert_eq!(write(&bfres, &BTreeMap::new()).unwrap(), file);
        // So does the model with its own geometry, bounds included (the synthetic file holds the game's values).
        let m = &bfres.models[0];
        let own = ShapeGeometry::from_shape(m, &m.shapes[0]).unwrap();
        let (boxes, spheres) = shape_bounds(m, &m.shapes[0], &own).unwrap();
        assert_eq!(boxes, m.shapes[0].boundings);
        assert_eq!(spheres, m.shapes[0].spheres);
        assert_eq!(write(&bfres, &BTreeMap::from([(0, own)])).unwrap(), file);
    }

    #[test]
    fn new_geometry_moves_the_tail_and_patches_everything() {
        let file = tiny_bfres();
        let bfres = Bfres::parse(&file, None).unwrap();
        let m = &bfres.models[0];
        // 300 vertices in a strip (6,000 bytes of vertex data: past the 4 KiB the GPU block had), all on Arm.
        let n = 300u32;
        let mut attributes = BTreeMap::new();
        attributes.insert(
            "_p0".to_owned(),
            (0..n)
                .map(|i| [i as f32 * 0.01, (i % 2) as f32, 0.0, 0.0])
                .collect(),
        );
        attributes.insert(
            "_u0".to_owned(),
            (0..n)
                .map(|i| [i as f32 / n as f32, 0.5, 0.0, 0.0])
                .collect(),
        );
        attributes.insert("_i0".to_owned(), vec![[1.0, 0.0, 0.0, 0.0]; n as usize]);
        attributes.insert("_w0".to_owned(), vec![[1.0, 0.0, 0.0, 0.0]; n as usize]);
        let indices: Vec<u32> = (0..n - 2).flat_map(|i| [i, i + 1, i + 2]).collect();
        let lod = LodGeometry {
            first_vertex: 0,
            sub_mesh_counts: vec![indices.len() as u32],
            indices,
        };
        let g = ShapeGeometry {
            vertex_count: n,
            attributes,
            lods: vec![lod],
        };
        let out = write(&bfres, &BTreeMap::from([(0, g.clone())])).unwrap();

        // The GPU block grew by 4 KiB; the memory pool block and the relocation table moved with it.
        assert_eq!(out.len(), file.len() + 0x1000);
        assert_eq!(out.as_slice().u32_at(0x1C).unwrap() as usize, out.len());
        let pool = file.as_slice().u64_at(0xA8).unwrap();
        assert_eq!(out.as_slice().u64_at(0xA8).unwrap(), pool + 0x1000);
        let rlt = out.as_slice().u32_at(0x18).unwrap() as usize;
        assert_eq!(&out[rlt..rlt + 4], b"_RLT");
        assert_eq!(out.as_slice().u32_at(rlt + 4).unwrap() as usize, rlt);
        let info = file.as_slice().u64_at(0xB0).unwrap() as usize;
        assert_eq!(out.as_slice().u32_at(info + 4).unwrap(), 0x2000);

        // It reads back as written.
        let back = Bfres::parse(&out, None).unwrap();
        let bm = &back.models[0];
        let read = ShapeGeometry::from_shape(bm, &bm.shapes[0]).unwrap();
        assert_eq!(read.vertex_count, n);
        assert_eq!(read.lods, g.lods);
        assert_eq!(read.attributes["_p0"], g.attributes["_p0"]);
        assert_eq!(read.attributes["_i0"], g.attributes["_i0"]);
        let (boxes, spheres) = shape_bounds(m, &m.shapes[0], &g).unwrap();
        assert_eq!(bm.shapes[0].boundings, boxes);
        assert_eq!(bm.shapes[0].spheres, spheres);
        // Arm's sphere is around the strip in Arm's space (x - 1).
        assert!(
            (spheres[1][0] - (2.99 / 2.0 - 1.0)).abs() < 1e-3,
            "{spheres:?}"
        );
        // And it writes back identically again.
        assert_eq!(write(&back, &BTreeMap::new()).unwrap(), out);
    }

    #[test]
    fn rejects_geometry_that_does_not_fit_the_shape() {
        let file = tiny_bfres();
        let bfres = Bfres::parse(&file, None).unwrap();
        let m = &bfres.models[0];
        let own = ShapeGeometry::from_shape(m, &m.shapes[0]).unwrap();
        let mut two_lods = own.clone();
        two_lods.lods.push(two_lods.lods[0].clone());
        assert!(write(&bfres, &BTreeMap::from([(0, two_lods)])).is_err());
        let mut missing = own.clone();
        missing.attributes.remove("_u0");
        assert!(write(&bfres, &BTreeMap::from([(0, missing)])).is_err());
        let mut past = own;
        past.lods[0].indices[0] = 7;
        assert!(write(&bfres, &BTreeMap::from([(0, past)])).is_err());
    }

    #[test]
    fn half_round_trips() {
        for h in 0..=0xFFFFu16 {
            let f = half_to_f32(h);
            if f.is_nan() {
                continue;
            }
            assert_eq!(f32_to_half(f), h, "{h:04x}");
        }
        assert_eq!(f32_to_half(1.0 + 1.0 / 4096.0), 0x3C00); // ties to even
        assert_eq!(f32_to_half(70000.0), 0x7C00);
    }

    #[test]
    fn attribute_encoding_inverts_decoding() {
        let cases: [(u32, &[u8]); 8] = [
            (0x1805, &[0, 0, 128, 63, 0, 0, 0, 192, 0, 0, 128, 62]),
            (0x1201, &[255, 255, 0, 128]),
            (0x1202, &[1, 128, 255, 127]),
            (0x0B01, &[1, 2, 3, 250]),
            (0x0B03, &[9, 0, 77, 255]),
            (0x0A03, &[0x40, 0x9C]),
            (0x1205, &[0x00, 0x3C, 0x55, 0xB5]),
            (0x0E02, &[0xFF, 0x01, 0xF0, 0xC1]),
        ];
        for (format, bytes) in cases {
            let f = AttribFormat(format);
            let v = f.decode(bytes).unwrap();
            let mut out = vec![0u8; bytes.len()];
            f.encode(v, &mut out).unwrap();
            assert_eq!(out, bytes, "format {}", f.name());
        }
    }

    #[test]
    fn smallest_sphere_of_simple_sets() {
        let s = smallest_sphere(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.5, 0.0]]);
        assert!((s[0] - 1.0).abs() < 1e-6 && s[1].abs() < 1e-6 && (s[3] - 1.0001).abs() < 1e-6);
        let corners: Vec<[f32; 3]> = (0..8)
            .map(|i| [(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32])
            .collect();
        let s = smallest_sphere(&corners);
        assert!((s[3] - (3f32.sqrt() / 2.0 + 0.0001)).abs() < 1e-5);
        assert_eq!(smallest_sphere(&[]), [0.0, 0.0, 0.0, 0.0001]);
        // Collinear and coplanar points have no single sphere through them.
        let line: Vec<[f32; 3]> = (0..10).map(|i| [i as f32, 0.0, 0.0]).collect();
        let s = smallest_sphere(&line);
        assert!(
            (s[0] - 4.5).abs() < 1e-5 && (s[3] - 4.5001).abs() < 1e-5,
            "{s:?}"
        );
        let square: Vec<[f32; 3]> = (0..4)
            .map(|i| [(i & 1) as f32, (i >> 1) as f32, 0.0])
            .collect();
        let s = smallest_sphere(&square);
        assert!((s[3] - (0.5f32.sqrt() + 0.0001)).abs() < 1e-5, "{s:?}");
    }
}
