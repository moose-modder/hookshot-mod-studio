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

use crate::bfres::{AttribFormat, Bfres, Bounding, Model, Shape, Skeleton};
use crate::bfres_bones::{self, NewBone};
use crate::bytes::ReadLe;
use crate::error::{Result, invalid, unsupported};
use crate::res_dict;

const INDEX_ALIGN: usize = 0x100;
const GPU_END_ALIGN: usize = 0x1000;
const SPHERE_MARGIN: f32 = 0.0001;
/// A bone's record in the skeleton's bone array.
const BONE_RECORD: usize = 0x58;

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
    write_edited(bfres, replace, &[], &BTreeMap::new())
}

/// Writes `bfres` (its first and only model) with `replace`'s shapes (by index) given new geometry, `new_bones`
/// added to its skeleton ([`bfres_bones::with_bones`]: the replaced shapes' vertices index the matrices of that
/// skeleton), and `skin_bones`' shapes given a new list of the bones they're skinned to (bone indices, ascending,
/// in that skeleton), their bounding spheres one per bone of it.
///
/// With bones added or lists changed, the new arrays (the bones' names, the bone array, the matrix list, the inverse
/// bind matrices, the name dictionary, the lists and their spheres) go after the file's own data, the skeleton's and
/// shapes' pointers patched; the GPU data starts on the next 4 KiB after them, and the rigidly skinned shapes'
/// vertices, which index the matrices past the smooth ones, have their indices moved up by the number of bones added.
/// The relocation table is written again with entries for every new pointer.
pub fn write_edited(
    bfres: &Bfres,
    replace: &BTreeMap<usize, ShapeGeometry>,
    new_bones: &[NewBone],
    skin_bones: &BTreeMap<usize, Vec<u16>>,
) -> Result<Vec<u8>> {
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
    let (data_section, index_section, vertex_section) =
        (rlt.sections[0], rlt.sections[1], rlt.sections[2]);
    if index_section.offset != gpu || vertex_section.offset != gpu + index_section.size {
        return Err(unsupported("GPU data not where the relocation table says"));
    }
    if data_section.offset != 0 || data_section.size > gpu {
        return Err(unsupported("the file's data not before its GPU data"));
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

    // The skeleton the written file has, and the shapes' lists of skin bones.
    let skeleton = if new_bones.is_empty() {
        model.skeleton.clone()
    } else {
        bfres_bones::with_bones(&model.skeleton, new_bones)?
    };
    let added = new_bones.len();
    let old_smooth = model.skeleton.smooth_count;
    for (&i, list) in skin_bones {
        let shape = model
            .shapes
            .get(i)
            .ok_or_else(|| invalid(format!("no shape {i} for a list of skin bones")))?;
        if shape.skin_count == 0 || list.is_empty() {
            return Err(invalid(format!(
                "shape {}: a list of skin bones for a shape without skinning, or an empty one",
                shape.name
            )));
        }
        if list.windows(2).any(|w| w[0] >= w[1])
            || list.iter().any(|&b| b as usize >= skeleton.bones.len())
        {
            return Err(invalid(format!(
                "shape {}: skin bones not ascending, or past the skeleton",
                shape.name
            )));
        }
    }
    // the rigid shapes whose own vertices are kept: their matrix indices move up past the new bones'
    let shifted_vb: BTreeSet<usize> = if added == 0 {
        BTreeSet::new()
    } else {
        model
            .shapes
            .iter()
            .filter(|s| s.skin_count == 1 && !replaced_vb.contains_key(&s.vertex_buffer))
            .map(|s| s.vertex_buffer)
            .collect()
    };

    // The data part: the file up to its GPU data; or with bones added or lists changed, the file's data and the new
    // arrays after it.
    let relocated: BTreeSet<usize> = rlt
        .entries_of(0)
        .iter()
        .flat_map(|e| e.pointers())
        .collect();
    let mut added_entries: Vec<Entry> = Vec::new();
    let mut sphere_arrays: BTreeMap<usize, usize> = BTreeMap::new();
    let grows = added > 0 || !skin_bones.is_empty();
    let (mut out, data_end, new_gpu) = if !grows {
        (d[..gpu].to_vec(), data_section.size, gpu)
    } else {
        let mut out = d[..data_section.size].to_vec();
        if added > 0 {
            append_skeleton(
                &mut out,
                d,
                model,
                &skeleton,
                new_bones,
                &relocated,
                &mut added_entries,
            )?;
        }
        // shapes' new lists of skin bones, and room for a sphere each (filled with the bounds below)
        for (&i, list) in skin_bones {
            let shape = &model.shapes[i];
            let list_at = alloc(&mut out, list.len() * 2, 8);
            for (k, &b) in list.iter().enumerate() {
                out[list_at + k * 2..list_at + k * 2 + 2].copy_from_slice(&b.to_le_bytes());
            }
            let spheres_at = alloc(&mut out, list.len() * 16, 8);
            point(
                &mut out,
                &relocated,
                &mut added_entries,
                shape.at + 0x20,
                list_at,
            );
            point(
                &mut out,
                &relocated,
                &mut added_entries,
                shape.at + 0x40,
                spheres_at,
            );
            out[shape.at + 0x58..shape.at + 0x5A]
                .copy_from_slice(&(list.len() as u16).to_le_bytes());
            sphere_arrays.insert(i, spheres_at);
        }
        let end = out.len();
        let new_gpu = end.next_multiple_of(GPU_END_ALIGN).max(gpu);
        out.resize(new_gpu, 0);
        (out, end, new_gpu)
    };

    let mut block: Vec<u8> = Vec::new();
    let place = |block: &mut Vec<u8>, bytes: &[u8], align: usize| -> usize {
        block.resize(
            (new_gpu + block.len()).next_multiple_of(align.max(1)) - new_gpu,
            0,
        );
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
    let index_end = (new_gpu + block.len()).next_multiple_of(INDEX_ALIGN) - new_gpu;
    block.resize(index_end, 0);

    // Vertex buffers, in order: each laid out for what it holds (a skin index past its format's widens it).
    for (vi, vb) in model.vertex_buffers.iter().enumerate() {
        let align = d.u16_at(vb.at + 0x56)? as usize;
        let size_array = d.offset_at(vb.at + 0x30)?;
        let stride_array = d.offset_at(vb.at + 0x38)?;
        let own = || {
            let mut attributes = BTreeMap::new();
            for a in &vb.attributes {
                attributes.insert(a.name.clone(), vb.read(a)?);
            }
            Ok::<_, crate::error::Error>(attributes)
        };
        let new_data = match replaced_vb.get(&vi) {
            Some(g) => {
                let layout = layout_for(vb, &|name| g.attributes.get(name).cloned())?;
                let data = encode_vertices(vb, g, &layout)?;
                write_layout(&mut out, vb, stride_array, &layout);
                Some(data)
            }
            None if shifted_vb.contains(&vi) => {
                // the game's own vertices, their matrix indices past the smooth ones moved up; in place, unless
                // that takes one past its format's
                let mut attributes = own()?;
                for name in ["_i0", "_i1"] {
                    if let Some(v) = attributes.get_mut(name) {
                        for x in v.iter_mut() {
                            for c in x.iter_mut() {
                                if *c as usize >= old_smooth {
                                    *c += added as f32;
                                }
                            }
                        }
                    }
                }
                let layout = layout_for(vb, &|name| attributes.get(name).cloned())?;
                if layout.strides == vb.buffers.iter().map(|b| b.stride).collect::<Vec<_>>()
                    && layout.formats == vb.attributes.iter().map(|a| a.format).collect::<Vec<_>>()
                {
                    Some(shift_rigid_indices(vb, old_smooth, added)?)
                } else {
                    let g = ShapeGeometry {
                        vertex_count: vb.vertex_count,
                        attributes,
                        lods: Vec::new(),
                    };
                    let data = encode_vertices(vb, &g, &layout)?;
                    write_layout(&mut out, vb, stride_array, &layout);
                    Some(data)
                }
            }
            None => None,
        };
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
    let gpu_end = (new_gpu + block.len()).next_multiple_of(GPU_END_ALIGN);
    block.resize(gpu_end - new_gpu, 0);

    // Bounds of the new shapes, and of shapes with a new list of skin bones.
    let own: BTreeMap<usize, ShapeGeometry> = skin_bones
        .keys()
        .filter(|i| !replace.contains_key(i))
        .map(|&i| Ok((i, ShapeGeometry::from_shape(model, &model.shapes[i])?)))
        .collect::<Result<_>>()?;
    for (&i, g) in replace.iter().chain(&own) {
        let shape = &model.shapes[i];
        let list = skin_bones
            .get(&i)
            .map_or(&shape.skin_bone_indices[..], |l| &l[..]);
        let (boxes, spheres) = shape_bounds(&skeleton, shape, list, g)?;
        // Only where the file has room for them (the arrays the reader found, or the new ones).
        if shape.bounding_at != 0 {
            for (k, b) in boxes.iter().enumerate().take(shape.boundings.len()) {
                put_f32s(&mut out, shape.bounding_at + k * 24, &b.center);
                put_f32s(&mut out, shape.bounding_at + k * 24 + 12, &b.extent);
            }
        }
        match sphere_arrays.get(&i) {
            Some(&at) => {
                for (k, s) in spheres.iter().enumerate().take(list.len()) {
                    put_f32s(&mut out, at + k * 16, s);
                }
            }
            None if shape.radius_at != 0 => {
                for (k, s) in spheres.iter().enumerate().take(shape.spheres.len()) {
                    put_f32s(&mut out, shape.radius_at + k * 16, s);
                }
            }
            None => {}
        }
    }

    // The GPU block, then what came after the game's (the memory pool block) moved by `delta`; then the table.
    let delta = gpu_end as i64 - old_gpu_end as i64;
    let shift = |v: usize| {
        if v >= old_gpu_end {
            (v as i64 + delta) as usize
        } else {
            v
        }
    };
    out.extend_from_slice(&block);
    out.extend_from_slice(&d[old_gpu_end..rlt.at]);

    // Each section where it is now: pointers into a section that moved follow it, and entries in the moved part
    // move with it.
    let new_sections: Vec<(usize, usize)> = rlt
        .sections
        .iter()
        .enumerate()
        .map(|(si, s)| match si {
            0 => (0, if grows { data_end } else { s.size }),
            1 => (new_gpu, index_end),
            2 => (new_gpu + index_end, gpu_end - new_gpu - index_end),
            _ => (shift(s.offset), s.size),
        })
        .collect();
    for (si, section) in rlt.sections.iter().enumerate() {
        let moved_by = new_sections[si].0 as i64 - section.offset as i64;
        if si == 0 || moved_by == 0 {
            continue;
        }
        for entry in rlt.entries_of(si) {
            for at in entry.pointers() {
                let at = shift(at);
                let v = out.as_slice().u64_at(at)?;
                if v != 0 {
                    put_u64(&mut out, at, (v as i64 + moved_by) as u64);
                }
            }
        }
    }
    let r = out.len();
    let mut table = Vec::new();
    table.extend_from_slice(b"_RLT");
    table.extend_from_slice(&(r as u32).to_le_bytes());
    table.extend_from_slice(&(rlt.sections.len() as u32).to_le_bytes());
    table.extend_from_slice(d.slice_at(rlt.at + 12, 4)?);
    let mut entries: Vec<Vec<Entry>> = (0..rlt.sections.len())
        .map(|si| {
            rlt.entries_of(si)
                .iter()
                .map(|e| Entry {
                    at: shift(e.at),
                    ..*e
                })
                .collect()
        })
        .collect();
    if !added_entries.is_empty() {
        entries[0].extend(added_entries);
        entries[0].sort_by_key(|e| e.at);
    }
    let mut first = 0usize;
    for (si, list) in entries.iter().enumerate() {
        table.extend_from_slice(d.slice_at(rlt.at + 0x10 + si * 0x18, 8)?);
        table.extend_from_slice(&(new_sections[si].0 as u32).to_le_bytes());
        table.extend_from_slice(&(new_sections[si].1 as u32).to_le_bytes());
        table.extend_from_slice(&(first as u32).to_le_bytes());
        table.extend_from_slice(&(list.len() as u32).to_le_bytes());
        first += list.len();
    }
    for e in entries.iter().flatten() {
        if e.structs > u16::MAX as usize
            || e.offsets > u8::MAX as usize
            || e.padding > u8::MAX as usize
        {
            return Err(invalid("a relocation entry too large to write"));
        }
        table.extend_from_slice(&(e.at as u32).to_le_bytes());
        table.extend_from_slice(&(e.structs as u16).to_le_bytes());
        table.extend_from_slice(&[e.offsets as u8, e.padding as u8]);
    }
    out.extend_from_slice(&table);
    put_u32(&mut out, 0x18, r as u32);

    // File size and the GPU buffer size.
    let len = out.len() as u32;
    put_u32(&mut out, 0x1C, len);
    if buffer_info != 0 {
        put_u32(&mut out, buffer_info + 4, (gpu_end - new_gpu) as u32);
    }
    Ok(out)
}

/// Room for `size` bytes at the end of `out`, aligned to `align`: its position.
fn alloc(out: &mut Vec<u8>, size: usize, align: usize) -> usize {
    let at = out.len().next_multiple_of(align);
    out.resize(at + size, 0);
    at
}

/// The pointer at `field` set to `to`, with a relocation entry for it unless the table lists it already.
fn point(
    out: &mut [u8],
    relocated: &BTreeSet<usize>,
    added_entries: &mut Vec<Entry>,
    field: usize,
    to: usize,
) {
    put_u64(out, field, to as u64);
    if !relocated.contains(&field) {
        added_entries.push(Entry {
            at: field,
            structs: 1,
            offsets: 1,
            padding: 0,
        });
    }
}

/// The skeleton `skeleton` (the model's with `new_bones` after its own, [`bfres_bones::with_bones`]) written at the
/// end of `out` (the file's data; `d` as it was): the new bones' names, the bone array (the game's records as they
/// are, their rigid indices as `skeleton` has them, then the new ones), the matrix list, the inverse bind matrices and
/// the name dictionary, the skeleton's pointers and counts patched; relocation entries for the new pointers into
/// `added_entries`.
fn append_skeleton(
    out: &mut Vec<u8>,
    d: &[u8],
    model: &Model,
    skeleton: &Skeleton,
    new_bones: &[NewBone],
    relocated: &BTreeSet<usize>,
    added_entries: &mut Vec<Entry>,
) -> Result<()> {
    let sk = model.skeleton.at;
    // the new bones' names, as the file keeps strings: a u16 length, the bytes, a zero
    let mut name_at = Vec::with_capacity(new_bones.len());
    for nb in new_bones {
        let at = alloc(out, 2 + nb.name.len() + 1, 2);
        out[at..at + 2].copy_from_slice(&(nb.name.len() as u16).to_le_bytes());
        out[at + 2..at + 2 + nb.name.len()].copy_from_slice(nb.name.as_bytes());
        name_at.push(at);
    }
    let old_count = model.skeleton.bones.len();
    let count = skeleton.bones.len();
    // the bone array: the game's bones as they are (their rigid indices moved up), then the new ones
    let bones_at = alloc(out, count * BONE_RECORD, 8);
    for (i, b) in model.skeleton.bones.iter().enumerate() {
        let at = bones_at + i * BONE_RECORD;
        out[at..at + BONE_RECORD].copy_from_slice(d.slice_at(b.at, BONE_RECORD)?);
        let rigid = skeleton.bones[i].rigid_matrix.map_or(-1, |r| r as i16);
        out[at + 0x26..at + 0x28].copy_from_slice(&rigid.to_le_bytes());
    }
    for (j, nb) in new_bones.iter().enumerate() {
        let b = &skeleton.bones[old_count + j];
        let at = bones_at + (old_count + j) * BONE_RECORD;
        put_u64(out, at, name_at[j] as u64);
        let index = (old_count + j) as u16;
        out[at + 0x20..at + 0x22].copy_from_slice(&index.to_le_bytes());
        out[at + 0x22..at + 0x24].copy_from_slice(&(nb.parent as i16).to_le_bytes());
        let smooth = b.smooth_matrix.map_or(-1, |s| s as i16);
        out[at + 0x24..at + 0x26].copy_from_slice(&smooth.to_le_bytes());
        out[at + 0x26..at + 0x28].copy_from_slice(&(-1i16).to_le_bytes());
        out[at + 0x28..at + 0x2A].copy_from_slice(&(-1i16).to_le_bytes());
        put_u32(out, at + 0x2C, b.flags);
        put_f32s(out, at + 0x30, &b.scale);
        put_f32s(out, at + 0x3C, &b.rotation);
        put_f32s(out, at + 0x4C, &b.translation);
    }
    added_entries.push(Entry {
        at: bones_at,
        structs: count,
        offsets: 3,
        padding: BONE_RECORD / 8 - 3,
    });
    let list_at = alloc(out, skeleton.matrix_to_bone.len() * 2, 8);
    for (k, &b) in skeleton.matrix_to_bone.iter().enumerate() {
        out[list_at + k * 2..list_at + k * 2 + 2].copy_from_slice(&b.to_le_bytes());
    }
    let inverse_at = alloc(out, skeleton.smooth_count * 48, 8);
    for (k, m) in skeleton.inverse_model_matrices.iter().enumerate() {
        put_f32s(out, inverse_at + k * 48, m);
    }
    // the name dictionary, built again; the root's key the game's (an empty string)
    let old_dict = d.offset_at(sk + 0x08)?;
    let names: Vec<&str> = skeleton.bones.iter().map(|b| b.name.as_str()).collect();
    let nodes = res_dict::build(&names);
    let dict_at = alloc(out, 8 + nodes.len() * 16, 8);
    out[dict_at..dict_at + 4].copy_from_slice(d.slice_at(old_dict, 4)?);
    put_u32(out, dict_at + 4, names.len() as u32);
    for (k, node) in nodes.iter().enumerate() {
        let at = dict_at + 8 + k * 16;
        put_u32(out, at, node.reference);
        out[at + 4..at + 6].copy_from_slice(&node.left.to_le_bytes());
        out[at + 6..at + 8].copy_from_slice(&node.right.to_le_bytes());
        let key = match k {
            0 => d.u64_at(old_dict + 16)?,
            _ if k <= old_count => d.u64_at(model.skeleton.bones[k - 1].at)?,
            _ => name_at[k - 1 - old_count] as u64,
        };
        put_u64(out, at + 8, key);
    }
    added_entries.push(Entry {
        at: dict_at + 16,
        structs: nodes.len(),
        offsets: 1,
        padding: 1,
    });
    point(out, relocated, added_entries, sk + 0x08, dict_at);
    point(out, relocated, added_entries, sk + 0x10, bones_at);
    point(out, relocated, added_entries, sk + 0x18, list_at);
    point(out, relocated, added_entries, sk + 0x20, inverse_at);
    out[sk + 0x38..sk + 0x3A].copy_from_slice(&(count as u16).to_le_bytes());
    out[sk + 0x3A..sk + 0x3C].copy_from_slice(&(skeleton.smooth_count as u16).to_le_bytes());
    Ok(())
}

/// Writes a file holding a skeleton alone (`bfres`: one model without shapes, nothing but its data in its relocation
/// table's first section: the `*_Animation.Skeleton` files beside some animation files) with `new_bones` added: the
/// skeleton written after the file's data as [`write_edited`] writes a model's, the relocation table after it.
pub fn write_skeleton(bfres: &Bfres, new_bones: &[NewBone]) -> Result<Vec<u8>> {
    let d = bfres.data;
    let [model] = bfres.models.as_slice() else {
        return Err(unsupported(format!(
            "{} models in one file",
            bfres.models.len()
        )));
    };
    if !model.shapes.is_empty() {
        return Err(invalid("a model with meshes: write_edited writes those"));
    }
    let rlt = Relocations::parse(d)?;
    let Some(first) = rlt.sections.first() else {
        return Err(invalid("a relocation table without sections"));
    };
    if first.offset != 0
        || rlt
            .sections
            .iter()
            .skip(1)
            .any(|s| s.size != 0 || s.entry_count != 0)
    {
        return Err(unsupported(
            "a skeleton file whose relocation table has more than its first section in use",
        ));
    }
    let skeleton = bfres_bones::with_bones(&model.skeleton, new_bones)?;
    let relocated: BTreeSet<usize> = rlt
        .entries_of(0)
        .iter()
        .flat_map(|e| e.pointers())
        .collect();
    let mut out = d[..first.size].to_vec();
    let mut added_entries = Vec::new();
    if !new_bones.is_empty() {
        append_skeleton(
            &mut out,
            d,
            model,
            &skeleton,
            new_bones,
            &relocated,
            &mut added_entries,
        )?;
    }
    let end = out.len();
    let r = end.next_multiple_of(8);
    out.resize(r, 0);
    let mut entries: Vec<Entry> = rlt.entries_of(0).to_vec();
    entries.extend(added_entries);
    entries.sort_by_key(|e| e.at);
    let mut table = Vec::new();
    table.extend_from_slice(b"_RLT");
    table.extend_from_slice(&(r as u32).to_le_bytes());
    table.extend_from_slice(&(rlt.sections.len() as u32).to_le_bytes());
    table.extend_from_slice(d.slice_at(rlt.at + 12, 4)?);
    for si in 0..rlt.sections.len() {
        table.extend_from_slice(d.slice_at(rlt.at + 0x10 + si * 0x18, 8)?);
        let (pos, size, first, count) = if si == 0 {
            (0, end, 0, entries.len())
        } else {
            (end, 0, entries.len(), 0)
        };
        for v in [pos, size, first, count] {
            table.extend_from_slice(&(v as u32).to_le_bytes());
        }
    }
    for e in &entries {
        if e.structs > u16::MAX as usize
            || e.offsets > u8::MAX as usize
            || e.padding > u8::MAX as usize
        {
            return Err(invalid("a relocation entry too large to write"));
        }
        table.extend_from_slice(&(e.at as u32).to_le_bytes());
        table.extend_from_slice(&(e.structs as u16).to_le_bytes());
        table.extend_from_slice(&[e.offsets as u8, e.padding as u8]);
    }
    out.extend_from_slice(&table);
    put_u32(&mut out, 0x18, r as u32);
    let len = out.len() as u32;
    put_u32(&mut out, 0x1C, len);
    Ok(out)
}

/// A vertex buffer's attribute records and strides patched to `layout`.
fn write_layout(
    out: &mut [u8],
    vb: &crate::bfres::VertexBuffer,
    stride_array: usize,
    layout: &Layout,
) {
    for (k, a) in vb.attributes.iter().enumerate() {
        put_u32(out, a.at + 0x08, layout.formats[k].0);
        out[a.at + 0x0C..a.at + 0x0E].copy_from_slice(&layout.offsets[k].to_le_bytes());
    }
    for (b, &s) in layout.strides.iter().enumerate() {
        put_u32(out, stride_array + b * 16, s);
    }
}

/// A rigid shape's vertex buffer as the game has it, its matrix indices past the smooth ones (`smooth`) moved up by
/// `added` (the bones added before them).
fn shift_rigid_indices(
    vb: &crate::bfres::VertexBuffer,
    smooth: usize,
    added: usize,
) -> Result<Vec<Vec<u8>>> {
    let mut buffers: Vec<Vec<u8>> = vb.buffers.iter().map(|b| b.data.to_vec()).collect();
    for a in vb.attributes.iter().filter(|a| a.name == "_i0") {
        let unknown = || unsupported(format!("attribute {}: format {}", a.name, a.format.name()));
        let size = a.format.size().ok_or_else(unknown)?;
        let stride = vb.buffers[a.buffer as usize].stride as usize;
        let buffer = &mut buffers[a.buffer as usize];
        for v in 0..vb.vertex_count as usize {
            let at = v * stride + a.offset as usize;
            let slot = &mut buffer[at..at + size];
            let mut value = a.format.decode(slot).ok_or_else(unknown)?;
            for c in value.iter_mut() {
                if *c as usize >= smooth {
                    *c += added as f32;
                }
            }
            a.format.encode(value, slot).ok_or_else(unknown)?;
        }
    }
    Ok(buffers)
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
/// A vertex buffer's layout: each attribute's format and offset, each buffer's stride.
#[derive(Debug, Clone, PartialEq)]
struct Layout {
    formats: Vec<AttribFormat>,
    offsets: Vec<u16>,
    strides: Vec<u32>,
}

/// The 16-bit unsigned format of an 8-bit one's components (skin indices: the game's own use these past 255).
fn wider(format: AttribFormat) -> Option<AttribFormat> {
    match format.0 {
        0x0203 => Some(AttribFormat(0x0A03)),
        0x0903 => Some(AttribFormat(0x1203)),
        0x0B03 => Some(AttribFormat(0x1503)),
        _ => None,
    }
}

/// The layout a vertex buffer needs for `values` (its attributes', by name): its own, unless a skin index
/// attribute (`_i0`, `_i1`) holds an index past what its format stores; then that attribute is 16 bits a
/// component, and its buffer's attributes are packed again in their order, each on 4 bytes, the stride too.
fn layout_for(
    vb: &crate::bfres::VertexBuffer,
    values: &dyn Fn(&str) -> Option<Vec<[f32; 4]>>,
) -> Result<Layout> {
    let mut layout = Layout {
        formats: vb.attributes.iter().map(|a| a.format).collect(),
        offsets: vb.attributes.iter().map(|a| a.offset).collect(),
        strides: vb.buffers.iter().map(|b| b.stride).collect(),
    };
    let mut widened_buffers = BTreeSet::new();
    for (k, a) in vb.attributes.iter().enumerate() {
        if a.name != "_i0" && a.name != "_i1" {
            continue;
        }
        let (Some(size), Some(n)) = (a.format.size(), a.format.components()) else {
            continue;
        };
        let largest = values(&a.name)
            .map(|v| {
                v.iter()
                    .flat_map(|x| x.iter().take(n).copied())
                    .fold(0f32, f32::max)
            })
            .unwrap_or(0.0);
        let max = ((1u64 << (8 * (size / n))) - 1) as f32;
        if largest <= max {
            continue;
        }
        let w = wider(a.format).ok_or_else(|| {
            unsupported(format!(
                "attribute {}: skin index {largest} past what {} holds",
                a.name,
                a.format.name()
            ))
        })?;
        if largest > 65535.0 {
            return Err(unsupported(format!(
                "attribute {}: skin index {largest} past 16 bits",
                a.name
            )));
        }
        layout.formats[k] = w;
        widened_buffers.insert(a.buffer);
    }
    for b in widened_buffers {
        let mut order: Vec<usize> = (0..vb.attributes.len())
            .filter(|&k| vb.attributes[k].buffer == b)
            .collect();
        order.sort_by_key(|&k| vb.attributes[k].offset);
        let mut at = 0usize;
        for k in order {
            let size = layout.formats[k].size().ok_or_else(|| {
                unsupported(format!(
                    "attribute {}: format {}",
                    vb.attributes[k].name,
                    layout.formats[k].name()
                ))
            })?;
            at = at.next_multiple_of(4);
            layout.offsets[k] = at as u16;
            at += size;
        }
        layout.strides[b as usize] = at.next_multiple_of(4) as u32;
    }
    Ok(layout)
}

fn encode_vertices(
    vb: &crate::bfres::VertexBuffer,
    g: &ShapeGeometry,
    layout: &Layout,
) -> Result<Vec<Vec<u8>>> {
    let mut buffers: Vec<Vec<u8>> = layout
        .strides
        .iter()
        .map(|&s| vec![0u8; s as usize * g.vertex_count as usize])
        .collect();
    for (k, a) in vb.attributes.iter().enumerate() {
        let values = &g.attributes[&a.name];
        let format = layout.formats[k];
        let size = format.size().ok_or_else(|| {
            unsupported(format!("attribute {}: format {}", a.name, format.name()))
        })?;
        let stride = layout.strides[a.buffer as usize] as usize;
        let offset = layout.offsets[k] as usize;
        let out = &mut buffers[a.buffer as usize];
        for (v, value) in values.iter().enumerate() {
            let at = v * stride + offset;
            format
                .encode(*value, &mut out[at..at + size])
                .ok_or_else(|| {
                    unsupported(format!("attribute {}: format {}", a.name, format.name()))
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
fn local_positions(
    skeleton: &Skeleton,
    shape: &Shape,
    g: &ShapeGeometry,
) -> Result<Vec<LocalPoint>> {
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
                (1, Some(i0)) => matrix_bone(skeleton, i0[v][0])?,
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
            let bone = matrix_bone(skeleton, i[v][k % 4])?;
            let m = skeleton.bones[bone]
                .smooth_matrix
                .and_then(|s| skeleton.inverse_model_matrices.get(s))
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

fn matrix_bone(skeleton: &Skeleton, index: f32) -> Result<usize> {
    skeleton
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

/// The shape's bounding boxes (per LOD: each sub-mesh, then the mesh) and spheres (per bone of `skin_bones`, the
/// shape's list of skin bones; or one without), computed the game's way, the vertices' matrices those of `skeleton`.
pub fn shape_bounds(
    skeleton: &Skeleton,
    shape: &Shape,
    skin_bones: &[u16],
    g: &ShapeGeometry,
) -> Result<(Vec<Bounding>, Vec<[f32; 4]>)> {
    let local = local_positions(skeleton, shape, g)?;
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
    let spheres = if skin_bones.is_empty() {
        // One sphere for the shape, around all of its vertices (in its bone's space).
        let all: Vec<[f32; 3]> = local.iter().map(|&(_, _, p, _)| p).collect();
        vec![smallest_sphere(&all)]
    } else {
        skin_bones
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
        let (boxes, spheres) = shape_bounds(
            &m.skeleton,
            &m.shapes[0],
            &m.shapes[0].skin_bone_indices,
            &own,
        )
        .unwrap();
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
        let (boxes, spheres) = shape_bounds(
            &m.skeleton,
            &m.shapes[0],
            &m.shapes[0].skin_bone_indices,
            &g,
        )
        .unwrap();
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
    fn a_new_bone_is_written_and_reads_back() {
        let file = tiny_bfres();
        let bfres = Bfres::parse(&file, None).unwrap();
        let m = &bfres.models[0];
        let hand = NewBone {
            name: "Hand".into(),
            parent: 1,
            scale: [1.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.5, 0.0, 0.0],
        };
        // vertex 1 moves onto Hand (its smooth matrix: 2, after the game's two)
        let mut g = ShapeGeometry::from_shape(m, &m.shapes[0]).unwrap();
        g.attributes.get_mut("_i0").unwrap()[1] = [2.0, 0.0, 0.0, 0.0];
        let out = write_edited(
            &bfres,
            &BTreeMap::from([(0, g.clone())]),
            std::slice::from_ref(&hand),
            &BTreeMap::from([(0, vec![0, 1, 2])]),
        )
        .unwrap();
        crate::bfres_anim_write::check_relocations(&out).unwrap();
        let back = Bfres::parse(&out, None).unwrap();
        let s = &back.models[0].skeleton;
        assert_eq!(s.bones.len(), 3);
        assert_eq!(s.bones[2].name, "Hand");
        assert_eq!(s.bones[2].parent, Some(1));
        assert_eq!(s.bones[2].smooth_matrix, Some(2));
        assert_eq!(s.matrix_to_bone, [0, 1, 2]);
        assert_eq!(s.smooth_count, 3);
        // Hand is 0.5 past Arm (at x = 1): its inverse bind takes (1.5, 0, 0) to its origin
        let inv = s.inverse_model_matrices[2];
        let x = inv[0] * 1.5 + inv[3];
        assert!(
            x.abs() < 1e-6 && inv[7].abs() < 1e-6 && inv[11].abs() < 1e-6,
            "{inv:?}"
        );
        // the name dictionary finds it
        let dict = out.as_slice().offset_at(s.at + 0x08).unwrap();
        let count = out.as_slice().u32_at(dict + 4).unwrap() as usize;
        let nodes: Vec<res_dict::DictNode> = (0..=count)
            .map(|i| res_dict::DictNode {
                reference: out.as_slice().u32_at(dict + 8 + i * 16).unwrap(),
                left: out.as_slice().u16_at(dict + 8 + i * 16 + 4).unwrap(),
                right: out.as_slice().u16_at(dict + 8 + i * 16 + 6).unwrap(),
            })
            .collect();
        let names: Vec<&str> = s.bones.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(res_dict::find(&nodes, &names, "Hand"), Some(2));
        assert_eq!(res_dict::find(&nodes, &names, "Arm"), Some(1));
        // the shape lists Hand, with a sphere for it around vertex 1 (at x = 1: half a unit before Hand, in its
        // space)
        let shape = &back.models[0].shapes[0];
        assert_eq!(shape.skin_bone_indices, [0, 1, 2]);
        assert_eq!(shape.spheres.len(), 3);
        let sphere = shape.spheres[2];
        assert!(
            (sphere[0] + 0.5).abs() < 1e-5 && sphere[1].abs() < 1e-5 && sphere[2].abs() < 1e-5,
            "{:?}",
            shape.spheres
        );
        let read = ShapeGeometry::from_shape(&back.models[0], shape).unwrap();
        assert_eq!(read.attributes["_i0"][1][0], 2.0);
        // written again as it is, the same bytes
        assert_eq!(write(&back, &BTreeMap::new()).unwrap(), out);
    }

    #[test]
    fn a_skin_index_past_its_format_widens_it() {
        let file = tiny_bfres();
        let bfres = Bfres::parse(&file, None).unwrap();
        let m = &bfres.models[0];
        // 300 bones, a chain on Arm: the last one's smooth matrix is 301, past the 8-bit indices' 255
        let bones: Vec<NewBone> = (0..300)
            .map(|i| NewBone {
                name: format!("Chain_{i}"),
                parent: if i == 0 { 1 } else { 2 + i - 1 },
                scale: [1.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [0.01, 0.0, 0.0],
            })
            .collect();
        let mut g = ShapeGeometry::from_shape(m, &m.shapes[0]).unwrap();
        g.attributes.get_mut("_i0").unwrap()[1] = [301.0, 0.0, 0.0, 0.0];
        let vb = &m.vertex_buffers[m.shapes[0].vertex_buffer];
        let old = vb.attribute("_i0").unwrap().format;
        let out = write_edited(
            &bfres,
            &BTreeMap::from([(0, g.clone())]),
            &bones,
            &BTreeMap::from([(0, vec![0, 301])]),
        )
        .unwrap();
        crate::bfres_anim_write::check_relocations(&out).unwrap();
        let back = Bfres::parse(&out, None).unwrap();
        let bm = &back.models[0];
        let bvb = &bm.vertex_buffers[bm.shapes[0].vertex_buffer];
        let i0 = bvb.attribute("_i0").unwrap();
        assert_eq!(old.0, 0x0903);
        assert_eq!(i0.format.0, 0x1203);
        let read = ShapeGeometry::from_shape(bm, &bm.shapes[0]).unwrap();
        assert_eq!(read.attributes["_i0"][1][0], 301.0);
        assert_eq!(read.attributes["_p0"], g.attributes["_p0"]);
        assert_eq!(read.attributes["_w0"], g.attributes["_w0"]);
        assert_eq!(bm.skeleton.matrix_to_bone[301], 301);
        assert!(bvb.buffers[i0.buffer as usize].stride > vb.buffers[i0.buffer as usize].stride);
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
