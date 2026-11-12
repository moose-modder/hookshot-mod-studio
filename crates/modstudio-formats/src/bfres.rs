//! BFRES model files (`Model/*.bfres.zs`), version 10 as used by this game.
//!
//! Ported from Syroot.NintenTools.Bfres as maintained in LegacySwitchLibraries: `ResFile.cs`, `Model/Model.cs`,
//! `Model/Skeleton/Skeleton.cs`, `Model/Skeleton/Bone.cs`, `Model/Shape/Shape.cs`, `Model/Shape/Mesh.cs`,
//! `Model/VertexBuffer.cs`, `Model/VertexAttrib.cs`, `Model/Material/MaterialParserV10.cs`, `Common/ResDict.cs`,
//! `Core/ResFileLoader.cs` (strings, including external string IDs).
//!
//! Only what a model mod needs is read: the skeleton, shapes with their meshes (LODs) and index buffers, vertex
//! buffers and their attributes, and each material's name and textures. Everything is read in place from the
//! file's bytes; [`Bfres`] keeps where each part lives so a writer can patch it.
//!
//! Attribute formats follow the console's graphics API: `channels << 8 | type` (see [`AttribFormat`]).

use std::collections::HashMap;

use crate::bytes::{ReadLe, check_magic};
use crate::error::{Result, invalid, unsupported};

/// External string flags (header byte 0xEE).
pub mod flags {
    /// Names may be IDs into the game's shared string table instead of offsets.
    pub const HAS_EXTERNAL_STRING: u8 = 1 << 1;
    /// This file is that table (`Shader/ExternalBinaryString.bfres`).
    pub const HOLDS_EXTERNAL_STRINGS: u8 = 1 << 2;
    /// GPU data follows the file instead of being inside it.
    pub const HAS_EXTERNAL_GPU: u8 = 1 << 3;
}

const HEADER_EXTERNAL_FLAGS: usize = 0xEE;
const MODEL_SIZE: usize = 0x78;
const BONE_SIZE: usize = 0x58;
const SHAPE_SIZE: usize = 0x60;
const MESH_SIZE: usize = 0x38;
const VERTEX_BUFFER_SIZE: usize = 0x58;
const ATTRIBUTE_SIZE: usize = 0x10;
const MATERIAL_SIZE: usize = 0xB0;

/// The game's shared string table: names that models store as 64-bit IDs.
#[derive(Debug, Clone, Default)]
pub struct ExternalStrings {
    by_id: HashMap<u64, String>,
}

impl ExternalStrings {
    /// From the decompressed contents of `Shader/ExternalBinaryString.bfres`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let header = Header::parse(data)?;
        if header.external_flags & flags::HOLDS_EXTERNAL_STRINGS == 0 {
            return Err(invalid("not an external string table (flag 0x04 not set)"));
        }
        let r = Reader {
            data,
            external: None,
            uses_external: false,
        };
        // In this file the fields after the buffer info are: the ID array, then a dictionary of the strings.
        let ids_at = data.offset_at(0xB8)?;
        let keys = r.dict_keys(0xC0)?;
        let mut by_id = HashMap::with_capacity(keys.len());
        for (i, key) in keys.into_iter().enumerate() {
            by_id.insert(data.u64_at(ids_at + i * 8)?, key);
        }
        Ok(ExternalStrings { by_id })
    }

    pub fn get(&self, id: u64) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// The fields of the file header this reader uses.
#[derive(Debug, Clone, Copy)]
struct Header {
    version: [u8; 4],
    external_flags: u8,
}

impl Header {
    fn parse(data: &[u8]) -> Result<Self> {
        check_magic(data, 0, "FRES")?;
        if data.u16_at(0x0C)? != 0xFEFF {
            return Err(unsupported("big-endian BFRES"));
        }
        let v = data.u32_at(0x08)?;
        let version = [(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8];
        if version[1] != 10 {
            return Err(unsupported(format!(
                "BFRES version {}.{}.{}.{} (only 10.x)",
                version[0], version[1], version[2], version[3]
            )));
        }
        Ok(Header {
            version,
            external_flags: data.u8_at(HEADER_EXTERNAL_FLAGS)?,
        })
    }
}

/// Reads strings and dictionaries, resolving external string IDs.
struct Reader<'a, 'e> {
    data: &'a [u8],
    external: Option<&'e ExternalStrings>,
    uses_external: bool,
}

impl Reader<'_, '_> {
    /// The string a 64-bit field at `field` points to (empty for a null field).
    fn string(&self, field: usize) -> Result<String> {
        let value = self.data.u64_at(field)?;
        if value == 0 {
            return Ok(String::new());
        }
        // A field that can't be an offset into the file is an ID into the external table.
        if self.uses_external && value >= self.data.len() as u64 {
            return Ok(match self.external.and_then(|e| e.get(value)) {
                Some(s) => s.to_owned(),
                None => format!("ExtStr_{value:016X}"),
            });
        }
        let at = value as usize;
        let len = self.data.u16_at(at)? as usize;
        let bytes = self.data.slice_at(at + 2, len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| invalid(format!("string at 0x{at:x} is not UTF-8")))
    }

    /// The keys of the dictionary a 64-bit field at `field` points to, in entry order.
    fn dict_keys(&self, field: usize) -> Result<Vec<String>> {
        let at = self.data.offset_at(field)?;
        if at == 0 {
            return Ok(Vec::new());
        }
        let count = self.data.i32_at(at + 4)?;
        let count = usize::try_from(count)
            .map_err(|_| invalid(format!("dictionary at 0x{at:x}: count {count}")))?;
        // Node 0 is the root; each node is 16 bytes: reference, left, right, key.
        (1..=count)
            .map(|i| self.string(at + 8 + i * 16 + 8))
            .collect()
    }
}

/// A parsed model file. Data borrows from the file's bytes.
#[derive(Debug)]
pub struct Bfres<'a> {
    pub data: &'a [u8],
    pub name: String,
    /// Major, major 2, minor, minor 2 (`[0, 10, 4, 0]` is 10.4).
    pub version: [u8; 4],
    pub external_flags: u8,
    /// Where the GPU data (vertex and index buffers) starts.
    pub buffer_offset: usize,
    pub models: Vec<Model<'a>>,
}

#[derive(Debug)]
pub struct Model<'a> {
    pub at: usize,
    pub name: String,
    pub skeleton: Skeleton,
    pub vertex_buffers: Vec<VertexBuffer<'a>>,
    pub shapes: Vec<Shape<'a>>,
    pub materials: Vec<Material>,
}

#[derive(Debug)]
pub struct Skeleton {
    pub at: usize,
    pub flags: u32,
    pub bones: Vec<Bone>,
    /// Matrix index to bone index: the smooth (skinned) matrices first, then the rigid ones.
    pub matrix_to_bone: Vec<u16>,
    pub smooth_count: usize,
    pub rigid_count: usize,
    /// Per smooth matrix: the inverse of the bone's model matrix, 3 rows of 4.
    pub inverse_model_matrices: Vec<[f32; 12]>,
}

impl Skeleton {
    /// Bone rotations are Euler angles (XYZ) rather than quaternions.
    pub fn euler_rotations(&self) -> bool {
        self.flags & 0x7000 == 0x1000
    }
}

#[derive(Debug, Clone)]
pub struct Bone {
    pub at: usize,
    pub name: String,
    pub parent: Option<usize>,
    pub smooth_matrix: Option<usize>,
    pub rigid_matrix: Option<usize>,
    pub flags: u32,
    pub scale: [f32; 3],
    /// A quaternion (x, y, z, w), or Euler angles (x, y, z, unused) when the skeleton says so.
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
}

#[derive(Debug)]
pub struct Shape<'a> {
    pub at: usize,
    pub name: String,
    pub flags: u32,
    pub material: usize,
    /// The bone the shape follows when it isn't skinned.
    pub bone: usize,
    pub vertex_buffer: usize,
    /// Influences per vertex: 0 (follows `bone`), 1 (rigid), more (smooth).
    pub skin_count: u8,
    pub skin_bone_indices: Vec<u16>,
    pub key_shape_count: u8,
    /// Level-of-detail meshes, most detailed first.
    pub meshes: Vec<Mesh<'a>>,
    /// Bounding boxes: one per sub-mesh and one per mesh (Syroot's count), at `bounding_at`.
    pub boundings: Vec<Bounding>,
    pub bounding_at: usize,
    /// Bounding spheres (centre, radius), at `radius_at`: one per skin bone, or one for the shape without skin bones
    /// (Syroot reads one per mesh there, which runs into the next structure in this game's files).
    pub spheres: Vec<[f32; 4]>,
    pub radius_at: usize,
}

/// An axis-aligned box: centre and half size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounding {
    pub center: [f32; 3],
    pub extent: [f32; 3],
}

#[derive(Debug)]
pub struct Mesh<'a> {
    pub at: usize,
    pub primitive: u32,
    /// 0 u8, 1 u16, 2 u32.
    pub index_format: u32,
    pub index_count: u32,
    /// Added to every index.
    pub first_vertex: u32,
    /// (first index, index count) of each sub-mesh.
    pub sub_meshes: Vec<(u32, u32)>,
    /// The index buffer.
    pub index_data: &'a [u8],
}

pub const PRIMITIVE_TRIANGLES: u32 = 0x03;

/// Shape flags.
pub mod shape_flags {
    pub const SUB_MESH_BOUNDARY_CONSISTENT: u32 = 1 << 1;
    pub const HAS_VERTEX_BUFFER: u32 = 1 << 2;
    /// The geometry is stored as meshlets, which this reader doesn't handle: no meshes, an empty vertex buffer.
    pub const MESHLET: u32 = 1 << 4;
}

impl Shape<'_> {
    pub fn is_meshlet(&self) -> bool {
        self.flags & shape_flags::MESHLET != 0
    }
}

impl Mesh<'_> {
    /// The indices with `first_vertex` added.
    pub fn indices(&self) -> Result<Vec<u32>> {
        let size = match self.index_format {
            0 => 1,
            1 => 2,
            2 => 4,
            f => return Err(unsupported(format!("index format {f}"))),
        };
        let count = self.index_count as usize;
        let data = self.index_data.slice_at(0, count * size)?;
        Ok((0..count)
            .map(|i| {
                let v = match size {
                    1 => u32::from(data[i]),
                    2 => u32::from(u16::from_le_bytes([data[2 * i], data[2 * i + 1]])),
                    _ => u32::from_le_bytes(data[4 * i..4 * i + 4].try_into().expect("4 bytes")),
                };
                v + self.first_vertex
            })
            .collect())
    }
}

#[derive(Debug)]
pub struct VertexBuffer<'a> {
    pub at: usize,
    pub vertex_count: u32,
    pub skin_count: u16,
    pub attributes: Vec<Attribute>,
    pub buffers: Vec<Buffer<'a>>,
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub at: usize,
    /// `_p0` position, `_n0` normal, `_t0` tangent, `_u0` UVs, `_c0` colours, `_i0`/`_w0` skin indices/weights...
    pub name: String,
    pub format: AttribFormat,
    /// Byte offset inside each vertex of the buffer.
    pub offset: u16,
    pub buffer: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct Buffer<'a> {
    pub stride: u32,
    pub data: &'a [u8],
}

impl VertexBuffer<'_> {
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }

    /// Every vertex's value of `attribute` (missing components 0).
    pub fn read(&self, attribute: &Attribute) -> Result<Vec<[f32; 4]>> {
        let buffer = self.buffers.get(attribute.buffer as usize).ok_or_else(|| {
            invalid(format!(
                "attribute {}: buffer {}",
                attribute.name, attribute.buffer
            ))
        })?;
        let size = attribute.format.size().ok_or_else(|| {
            unsupported(format!(
                "attribute {}: format {}",
                attribute.name,
                attribute.format.name()
            ))
        })?;
        (0..self.vertex_count as usize)
            .map(|i| {
                let at = i * buffer.stride as usize + attribute.offset as usize;
                let bytes = buffer.data.slice_at(at, size)?;
                attribute.format.decode(bytes).ok_or_else(|| {
                    unsupported(format!(
                        "attribute {}: format {}",
                        attribute.name,
                        attribute.format.name()
                    ))
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct Material {
    pub at: usize,
    pub name: String,
    /// Texture names, in the material's slot order.
    pub textures: Vec<String>,
    /// Sampler names (`_a0`, `_n0`, ...), in slot order.
    pub samplers: Vec<String>,
}

impl<'a> Bfres<'a> {
    /// Parses a decompressed model file. `external` resolves names stored as IDs (files with flag 0x02); without
    /// it such names read as `ExtStr_<id>`.
    pub fn parse(data: &'a [u8], external: Option<&ExternalStrings>) -> Result<Self> {
        let header = Header::parse(data)?;
        if header.external_flags & flags::HAS_EXTERNAL_GPU != 0 {
            return Err(unsupported("GPU data outside the file (flag 0x08)"));
        }
        if header.external_flags & flags::HOLDS_EXTERNAL_STRINGS != 0 {
            return Err(invalid("this is the external string table, not a model"));
        }
        let r = Reader {
            data,
            external,
            uses_external: header.external_flags & flags::HAS_EXTERNAL_STRING != 0,
        };
        let name = r.string(0x20)?;
        let buffer_info = data.offset_at(0xB0)?;
        let buffer_offset = if buffer_info == 0 {
            0
        } else {
            data.offset_at(buffer_info + 8)?
        };
        let model_array = data.offset_at(0x28)?;
        let model_count = data.u16_at(0xDC)? as usize;
        let models = (0..model_count)
            .map(|i| Model::parse(&r, model_array + i * MODEL_SIZE, buffer_offset))
            .collect::<Result<Vec<_>>>()?;
        Ok(Bfres {
            data,
            name,
            version: header.version,
            external_flags: header.external_flags,
            buffer_offset,
            models,
        })
    }
}

impl<'a> Model<'a> {
    fn parse(r: &Reader<'a, '_>, at: usize, buffer_offset: usize) -> Result<Self> {
        let d = r.data;
        check_magic(d, at, "FMDL")?;
        let name = r.string(at + 0x08)?;
        let skeleton = Skeleton::parse(r, d.offset_at(at + 0x18)?)?;
        let vertex_array = d.offset_at(at + 0x20)?;
        let shape_array = d.offset_at(at + 0x28)?;
        let material_array = d.offset_at(at + 0x38)?;
        let vertex_buffer_count = d.u16_at(at + 0x68)? as usize;
        let shape_count = d.u16_at(at + 0x6A)? as usize;
        let material_count = d.u16_at(at + 0x6C)? as usize;

        let vertex_buffers = (0..vertex_buffer_count)
            .map(|i| VertexBuffer::parse(r, vertex_array + i * VERTEX_BUFFER_SIZE, buffer_offset))
            .collect::<Result<Vec<_>>>()?;
        let shapes = (0..shape_count)
            .map(|i| Shape::parse(r, shape_array + i * SHAPE_SIZE, buffer_offset))
            .collect::<Result<Vec<_>>>()?;
        let materials = (0..material_count)
            .map(|i| Material::parse(r, material_array + i * MATERIAL_SIZE))
            .collect::<Result<Vec<_>>>()?;

        for s in &shapes {
            if s.vertex_buffer >= vertex_buffers.len() || s.material >= materials.len() {
                return Err(invalid(format!(
                    "shape {}: vertex buffer or material out of range",
                    s.name
                )));
            }
        }
        Ok(Model {
            at,
            name,
            skeleton,
            vertex_buffers,
            shapes,
            materials,
        })
    }
}

fn index(v: i16) -> Option<usize> {
    usize::try_from(v).ok()
}

impl Skeleton {
    fn parse(r: &Reader<'_, '_>, at: usize) -> Result<Self> {
        let d = r.data;
        check_magic(d, at, "FSKL")?;
        let flags = d.u32_at(at + 0x04)?;
        let bone_array = d.offset_at(at + 0x10)?;
        let matrix_list = d.offset_at(at + 0x18)?;
        let inverse_matrices = d.offset_at(at + 0x20)?;
        let bone_count = d.u16_at(at + 0x38)? as usize;
        let smooth_count = d.u16_at(at + 0x3A)? as usize;
        let rigid_count = d.u16_at(at + 0x3C)? as usize;

        let mut bones = Vec::with_capacity(bone_count);
        for i in 0..bone_count {
            let b = bone_array + i * BONE_SIZE;
            let f = |o: usize| d.f32_at(b + o);
            bones.push(Bone {
                at: b,
                name: r.string(b)?,
                parent: index(d.i16_at(b + 0x22)?),
                smooth_matrix: index(d.i16_at(b + 0x24)?),
                rigid_matrix: index(d.i16_at(b + 0x26)?),
                flags: d.u32_at(b + 0x2C)?,
                scale: [f(0x30)?, f(0x34)?, f(0x38)?],
                rotation: [f(0x3C)?, f(0x40)?, f(0x44)?, f(0x48)?],
                translation: [f(0x4C)?, f(0x50)?, f(0x54)?],
            });
        }
        for (i, b) in bones.iter().enumerate() {
            if b.parent.is_some_and(|p| p >= bone_count) {
                return Err(invalid(format!(
                    "bone {i} ({}): parent out of range",
                    b.name
                )));
            }
        }
        let matrix_to_bone = (0..smooth_count + rigid_count)
            .map(|i| d.u16_at(matrix_list + i * 2))
            .collect::<Result<Vec<_>>>()?;
        if matrix_to_bone.iter().any(|&b| b as usize >= bone_count) {
            return Err(invalid("skeleton: matrix list names a missing bone"));
        }
        let inverse_model_matrices = (0..smooth_count)
            .map(|i| {
                let m = inverse_matrices + i * 48;
                let mut out = [0f32; 12];
                for (k, v) in out.iter_mut().enumerate() {
                    *v = d.f32_at(m + k * 4)?;
                }
                Ok(out)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Skeleton {
            at,
            flags,
            bones,
            matrix_to_bone,
            smooth_count,
            rigid_count,
            inverse_model_matrices,
        })
    }
}

impl<'a> Shape<'a> {
    fn parse(r: &Reader<'a, '_>, at: usize, buffer_offset: usize) -> Result<Self> {
        let d = r.data;
        check_magic(d, at, "FSHP")?;
        let name = r.string(at + 0x08)?;
        let mesh_array = d.offset_at(at + 0x18)?;
        let skin_list = d.offset_at(at + 0x20)?;
        let skin_bone_count = d.u16_at(at + 0x58)? as usize;
        let mesh_count = d.u8_at(at + 0x5B)? as usize;

        let meshes = (0..mesh_count)
            .map(|i| Mesh::parse(d, mesh_array + i * MESH_SIZE, buffer_offset))
            .collect::<Result<Vec<_>>>()
            .map_err(|e| invalid(format!("shape {name}: {e}")))?;
        let skin_bone_indices = (0..skin_bone_count)
            .map(|i| d.u16_at(skin_list + i * 2))
            .collect::<Result<Vec<_>>>()?;
        let vec3 = |o: usize| -> Result<[f32; 3]> {
            Ok([d.f32_at(o)?, d.f32_at(o + 4)?, d.f32_at(o + 8)?])
        };
        let bounding_at = d.offset_at(at + 0x38)?;
        let bounding_count: usize = meshes.iter().map(|m| m.sub_meshes.len() + 1).sum();
        let boundings = if bounding_at == 0 {
            Vec::new()
        } else {
            (0..bounding_count)
                .map(|i| {
                    let b = bounding_at + i * 24;
                    Ok(Bounding {
                        center: vec3(b)?,
                        extent: vec3(b + 12)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        };
        let radius_at = d.offset_at(at + 0x40)?;
        let sphere_count = skin_bone_count.max(1);
        let spheres = if radius_at == 0 || mesh_count == 0 {
            Vec::new()
        } else {
            (0..sphere_count)
                .map(|i| {
                    let s = radius_at + i * 16;
                    Ok([
                        d.f32_at(s)?,
                        d.f32_at(s + 4)?,
                        d.f32_at(s + 8)?,
                        d.f32_at(s + 12)?,
                    ])
                })
                .collect::<Result<Vec<_>>>()?
        };
        Ok(Shape {
            boundings,
            bounding_at,
            spheres,
            radius_at,
            at,
            flags: d.u32_at(at + 0x04)?,
            material: d.u16_at(at + 0x52)? as usize,
            bone: d.u16_at(at + 0x54)? as usize,
            vertex_buffer: d.u16_at(at + 0x56)? as usize,
            skin_count: d.u8_at(at + 0x5A)?,
            key_shape_count: d.u8_at(at + 0x5C)?,
            skin_bone_indices,
            meshes,
            name,
        })
    }
}

impl<'a> Mesh<'a> {
    fn parse(d: &'a [u8], at: usize, buffer_offset: usize) -> Result<Self> {
        let sub_mesh_array = d.offset_at(at)?;
        let size_info = d.offset_at(at + 0x18)?;
        let face_offset = d.u32_at(at + 0x20)? as usize;
        let sub_mesh_count = d.u16_at(at + 0x34)? as usize;
        let buffer_size = d.u32_at(size_info)? as usize;
        let sub_meshes = (0..sub_mesh_count)
            .map(|i| {
                Ok((
                    d.u32_at(sub_mesh_array + i * 8)?,
                    d.u32_at(sub_mesh_array + i * 8 + 4)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Mesh {
            at,
            primitive: d.u32_at(at + 0x24)?,
            index_format: d.u32_at(at + 0x28)?,
            index_count: d.u32_at(at + 0x2C)?,
            first_vertex: d.u32_at(at + 0x30)?,
            sub_meshes,
            index_data: d.slice_at(buffer_offset + face_offset, buffer_size)?,
        })
    }
}

impl<'a> VertexBuffer<'a> {
    fn parse(r: &Reader<'a, '_>, at: usize, buffer_offset: usize) -> Result<Self> {
        let d = r.data;
        // Shapes made of meshlets leave their vertex buffer slot empty (zeros).
        if d.u32_at(at)? == 0 {
            return Ok(VertexBuffer {
                at,
                vertex_count: 0,
                skin_count: 0,
                attributes: Vec::new(),
                buffers: Vec::new(),
            });
        }
        check_magic(d, at, "FVTX")?;
        let attribute_array = d.offset_at(at + 0x08)?;
        let size_array = d.offset_at(at + 0x30)?;
        let stride_array = d.offset_at(at + 0x38)?;
        let data_offset = d.u32_at(at + 0x48)? as usize;
        let attribute_count = d.u8_at(at + 0x4C)? as usize;
        let buffer_count = d.u8_at(at + 0x4D)? as usize;
        let vertex_count = d.u32_at(at + 0x50)?;
        let skin_count = d.u16_at(at + 0x54)?;
        let alignment = d.u16_at(at + 0x56)? as usize;

        let mut attributes = Vec::with_capacity(attribute_count);
        for i in 0..attribute_count {
            let a = attribute_array + i * ATTRIBUTE_SIZE;
            attributes.push(Attribute {
                at: a,
                name: r.string(a)?,
                format: AttribFormat(d.u32_at(a + 0x08)?),
                offset: d.u16_at(a + 0x0C)?,
                buffer: d.u16_at(a + 0x0E)?,
            });
        }
        let mut buffers = Vec::with_capacity(buffer_count);
        let mut pos = buffer_offset + data_offset;
        for i in 0..buffer_count {
            if alignment > 1 {
                pos = pos.next_multiple_of(alignment);
            }
            let size = d.u32_at(size_array + i * 16)? as usize;
            let stride = d.u32_at(stride_array + i * 16)?;
            buffers.push(Buffer {
                stride,
                data: d.slice_at(pos, size)?,
            });
            pos += size;
        }
        Ok(VertexBuffer {
            at,
            vertex_count,
            skin_count,
            attributes,
            buffers,
        })
    }
}

impl Material {
    fn parse(r: &Reader<'_, '_>, at: usize) -> Result<Self> {
        let d = r.data;
        check_magic(d, at, "FMAT")?;
        let name = r.string(at + 0x08)?;
        let texture_names = d.offset_at(at + 0x20)?;
        let texture_count = d.u8_at(at + 0xA3)? as usize;
        let textures = (0..texture_count)
            .map(|i| r.string(texture_names + i * 8))
            .collect::<Result<Vec<_>>>()?;
        let samplers = r.dict_keys(at + 0x38)?;
        Ok(Material {
            at,
            name,
            textures,
            samplers,
        })
    }
}

/// A vertex attribute format: `channels << 8 | type`, as in the console's graphics API.
///
/// Channels: 0x02 R8, 0x09 R8G8, 0x0A R16, 0x0B R8G8B8A8, 0x0E R10G10B10A2, 0x12 R16G16, 0x14 R32,
/// 0x15 R16G16B16A16, 0x17 R32G32, 0x18 R32G32B32, 0x19 R32G32B32A32. Types: 1 unorm, 2 snorm, 3 uint, 4 sint,
/// 5 float, 8 uint read as float, 9 sint read as float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttribFormat(pub u32);

impl AttribFormat {
    pub fn channels(self) -> u32 {
        (self.0 >> 8) & 0xFF
    }

    pub fn kind(self) -> u32 {
        self.0 & 0xFF
    }

    /// (component count, bits per component) of the channel layout.
    fn layout(self) -> Option<(usize, usize)> {
        Some(match self.channels() {
            0x02 => (1, 8),
            0x09 => (2, 8),
            0x0A => (1, 16),
            0x0B => (4, 8),
            0x0E => (4, 0), // 10, 10, 10, 2
            0x12 => (2, 16),
            0x14 => (1, 32),
            0x15 => (4, 16),
            0x17 => (2, 32),
            0x18 => (3, 32),
            0x19 => (4, 32),
            _ => return None,
        })
    }

    /// Bytes per vertex.
    pub fn size(self) -> Option<usize> {
        let (n, bits) = self.layout()?;
        Some(if bits == 0 { 4 } else { n * bits / 8 })
    }

    /// Number of components.
    pub fn components(self) -> Option<usize> {
        self.layout().map(|(n, _)| n)
    }

    /// e.g. `32_32_32_Float`, `8_8_8_8_UNorm`, `16_UInt`.
    pub fn name(self) -> String {
        let channels = match self.layout() {
            Some((_, 0)) => "10_10_10_2".to_owned(),
            Some((n, bits)) => vec![bits.to_string(); n].join("_"),
            None => format!("channels{:02x}", self.channels()),
        };
        let kind = match self.kind() {
            1 => "UNorm",
            2 => "SNorm",
            3 => "UInt",
            4 => "SInt",
            5 => "Float",
            8 => "UIntToFloat",
            9 => "SIntToFloat",
            _ => "?",
        };
        format!("{channels}_{kind}")
    }

    /// One vertex's value.
    pub fn decode(self, b: &[u8]) -> Option<[f32; 4]> {
        let (n, bits) = self.layout()?;
        let kind = self.kind();
        let mut out = [0f32; 4];
        if bits == 0 {
            let v = u32::from_le_bytes(b.get(..4)?.try_into().ok()?);
            let unsigned = [v & 0x3FF, (v >> 10) & 0x3FF, (v >> 20) & 0x3FF, v >> 30];
            let signed = [
                ((v << 22) as i32) >> 22,
                ((v << 12) as i32) >> 22,
                ((v << 2) as i32) >> 22,
                (v as i32) >> 30,
            ];
            for c in 0..4 {
                let max = if c == 3 { 3.0 } else { 1023.0 };
                out[c] = match kind {
                    1 => unsigned[c] as f32 / max,
                    // The 2-bit w is read as -2..=1 (Syroot does the same).
                    2 if c == 3 => signed[c] as f32,
                    2 => (signed[c] as f32 / 511.0).max(-1.0),
                    3 | 8 => unsigned[c] as f32,
                    4 | 9 => signed[c] as f32,
                    _ => return None,
                };
            }
            return Some(out);
        }
        let bytes = bits / 8;
        for (c, slot) in out.iter_mut().enumerate().take(n) {
            let raw = b.get(c * bytes..(c + 1) * bytes)?;
            *slot = match (bits, kind) {
                (8, 1) => f32::from(raw[0]) / 255.0,
                (8, 2) => (f32::from(raw[0] as i8) / 127.0).max(-1.0),
                (8, 3 | 8) => f32::from(raw[0]),
                (8, 4 | 9) => f32::from(raw[0] as i8),
                (16, _) => {
                    let u = u16::from_le_bytes([raw[0], raw[1]]);
                    match kind {
                        1 => f32::from(u) / 65535.0,
                        2 => (f32::from(u as i16) / 32767.0).max(-1.0),
                        3 | 8 => f32::from(u),
                        4 | 9 => f32::from(u as i16),
                        5 => half_to_f32(u),
                        _ => return None,
                    }
                }
                (32, _) => {
                    let u = u32::from_le_bytes(raw.try_into().ok()?);
                    match kind {
                        3 | 8 => u as f32,
                        4 | 9 => u as i32 as f32,
                        5 => f32::from_bits(u),
                        _ => return None,
                    }
                }
                _ => return None,
            };
        }
        Some(out)
    }
}

/// IEEE 754 half precision to single.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1F);
    let mant = u32::from(h & 0x3FF);
    let bits = match (exp, mant) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalise.
            let shift = m.leading_zeros() - 21;
            sign | ((113 - shift) << 23) | (((m << shift) & 0x3FF) << 13)
        }
        (0x1F, m) => sign | 0x7F80_0000 | (m << 13),
        (e, m) => sign | ((e + 112) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_synthetic_model() {
        let file = crate::testing::tiny_bfres();
        let bfres = Bfres::parse(&file, None).unwrap();
        assert_eq!(
            (bfres.name.as_str(), bfres.version),
            ("tiny", [0, 10, 4, 0])
        );
        let m = &bfres.models[0];
        assert_eq!(m.name, "Tiny");

        let names: Vec<&str> = m.skeleton.bones.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Root", "Arm"]);
        assert_eq!(m.skeleton.bones[1].parent, Some(0));
        assert_eq!(m.skeleton.bones[1].translation, [1., 0., 0.]);
        assert_eq!(m.skeleton.matrix_to_bone, [0, 1]);
        assert_eq!(m.skeleton.inverse_model_matrices[1][3], -1.0);

        let shape = &m.shapes[0];
        assert_eq!((shape.name.as_str(), shape.skin_count), ("Tri__Mt_Test", 2));
        assert!(!shape.is_meshlet());
        assert_eq!(shape.meshes[0].indices().unwrap(), [0, 1, 2]);
        assert_eq!(shape.meshes[0].sub_meshes, [(0, 3)]);

        let vb = &m.vertex_buffers[shape.vertex_buffer];
        let pos = vb.read(vb.attribute("_p0").unwrap()).unwrap();
        assert_eq!(pos[1], [1., 0., 0., 0.]);
        let uv = vb.read(vb.attribute("_u0").unwrap()).unwrap();
        assert_eq!(uv[2], [0., 1., 0., 0.]);
        let bones = vb.read(vb.attribute("_i0").unwrap()).unwrap();
        assert_eq!(bones.iter().map(|v| v[0]).collect::<Vec<_>>(), [0., 1., 0.]);
        let weights = vb.read(vb.attribute("_w0").unwrap()).unwrap();
        assert_eq!(weights[0], [1., 0., 0., 0.]);

        let mat = &m.materials[0];
        assert_eq!(
            (
                mat.name.as_str(),
                mat.textures.as_slice(),
                mat.samplers.as_slice()
            ),
            (
                "Mt_Test",
                &["Tex_Test_Alb".to_owned()][..],
                &["_a0".to_owned()][..]
            )
        );
    }

    #[test]
    fn rejects_damaged_models() {
        let file = crate::testing::tiny_bfres();
        assert!(Bfres::parse(&file[..0x100], None).is_err());
        let mut v9 = file.clone();
        v9[0x0A] = 9;
        assert!(Bfres::parse(&v9, None).is_err());
        let mut bad = file;
        bad[0xF0] = b'X'; // FMDL signature
        assert!(Bfres::parse(&bad, None).is_err());
    }

    #[test]
    fn half_floats() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x3555), 0.333_251_95);
        assert_eq!(half_to_f32(0x0001), 5.960_464_5e-8);
        assert_eq!(half_to_f32(0x7BFF), 65504.0);
        assert!(half_to_f32(0x7C00).is_infinite());
        assert_eq!(half_to_f32(0x8000).to_bits(), 0x8000_0000);
    }

    #[test]
    fn attribute_formats() {
        let f = AttribFormat(0x1805); // 32_32_32 float
        assert_eq!(
            (f.size(), f.components(), f.name().as_str()),
            (Some(12), Some(3), "32_32_32_Float")
        );
        let mut b = Vec::new();
        for v in [1.5f32, -2.0, 0.25] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(f.decode(&b), Some([1.5, -2.0, 0.25, 0.0]));

        // 16-bit unsigned bone indices (Syroot calls this 16_SNorm).
        let f = AttribFormat(0x0A03);
        assert_eq!(f.name(), "16_UInt");
        assert_eq!(
            f.decode(&40000u16.to_le_bytes()),
            Some([40000.0, 0.0, 0.0, 0.0])
        );

        // 10_10_10_2 snorm: x = 511 (1.0), y = -511 (-1.0), z = 0, w = -1.
        let v: u32 = 511 | ((-511i32 as u32 & 0x3FF) << 10) | (3 << 30);
        let f = AttribFormat(0x0E02);
        assert_eq!(f.name(), "10_10_10_2_SNorm");
        assert_eq!(f.decode(&v.to_le_bytes()), Some([1.0, -1.0, 0.0, -1.0]));

        let f = AttribFormat(0x1201); // 16_16 unorm
        let mut b = 65535u16.to_le_bytes().to_vec();
        b.extend_from_slice(&0u16.to_le_bytes());
        assert_eq!(f.decode(&b), Some([1.0, 0.0, 0.0, 0.0]));

        let f = AttribFormat(0x0B03); // 8_8_8_8 uint
        assert_eq!(f.decode(&[1, 2, 3, 250]), Some([1.0, 2.0, 3.0, 250.0]));
        assert_eq!(
            AttribFormat(0x0B01).decode(&[255, 0, 51, 255]),
            Some([1.0, 0.0, 0.2, 1.0])
        );
        assert_eq!(AttribFormat(0x7701).decode(&[0; 16]), None);
    }
}
