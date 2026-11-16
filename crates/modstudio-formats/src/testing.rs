//! Small synthetic game files for tests (feature `testing`): nothing here comes from the game.

use crate::tegra::{Gob, Surface};

/// A growing file with helpers to place structures and patch fields.
struct Builder {
    data: Vec<u8>,
}

impl Builder {
    fn alloc(&mut self, size: usize, align: usize) -> usize {
        let at = self.data.len().next_multiple_of(align);
        self.data.resize(at + size, 0);
        at
    }
    fn bytes(&mut self, at: usize, b: &[u8]) {
        self.data[at..at + b.len()].copy_from_slice(b);
    }
    fn u8(&mut self, at: usize, v: u8) {
        self.data[at] = v;
    }
    fn u16(&mut self, at: usize, v: u16) {
        self.bytes(at, &v.to_le_bytes());
    }
    fn i16(&mut self, at: usize, v: i16) {
        self.bytes(at, &v.to_le_bytes());
    }
    fn u32(&mut self, at: usize, v: u32) {
        self.bytes(at, &v.to_le_bytes());
    }
    fn u64(&mut self, at: usize, v: u64) {
        self.bytes(at, &v.to_le_bytes());
    }
    fn f32s(&mut self, at: usize, v: &[f32]) {
        for (i, x) in v.iter().enumerate() {
            self.bytes(at + i * 4, &x.to_le_bytes());
        }
    }
    /// A u16-length-prefixed, zero-terminated string; returns its offset.
    fn string(&mut self, s: &str) -> usize {
        let at = self.alloc(2 + s.len() + 1, 2);
        self.u16(at, s.len() as u16);
        self.bytes(at + 2, s.as_bytes());
        at
    }
    /// A dictionary of `keys` (root node first); returns its offset.
    fn dict(&mut self, keys: &[&str]) -> usize {
        let at = self.alloc(8 + (keys.len() + 1) * 16, 8);
        self.u32(at + 4, keys.len() as u32);
        for (i, k) in keys.iter().enumerate() {
            let s = self.string(k) as u64;
            self.u64(at + 8 + (i + 1) * 16 + 8, s);
        }
        at
    }
}

/// A BFRES 10.4 file with one model: bones `Root` and `Arm` (at x = 1, child of `Root`), one shape `Tri__Mt_Test`
/// (3 vertices, 1 triangle, 2 influences per vertex) and one material `Mt_Test` using texture `Tex_Test_Alb`.
///
/// Vertices: positions (0,0,0), (1,0,0), (0,1,0); UVs (0,0), (1,0), (0,1); vertex 1 fully on `Arm`, the
/// others fully on `Root` (bone matrix indices 0 and 1 map to bones 0 and 1).
pub fn tiny_bfres() -> Vec<u8> {
    build_tiny(None)
}

/// [`tiny_bfres`] with a colour set `_c0` stored as 16-bit floats (the way some game meshes keep data in vertex
/// colours: values above 1 and below 0), `colours` per vertex (rounded to 16-bit floats).
pub fn tiny_bfres_coloured(colours: [[f32; 4]; 3]) -> Vec<u8> {
    build_tiny(Some(colours))
}

/// A 16-bit float's bits (round to nearest; normal numbers and zero only, enough for tests).
pub fn f16_bits(x: f32) -> u16 {
    if x == 0.0 {
        return 0;
    }
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    let mant = bits & 0x7F_FFFF;
    let rounded = (mant + 0x1000) >> 13;
    let (exp, rounded) = if rounded == 0x400 {
        (exp + 1, 0)
    } else {
        (exp, rounded)
    };
    sign | ((exp as u16) << 10) | rounded as u16
}

fn build_tiny(colours: Option<[[f32; 4]; 3]>) -> Vec<u8> {
    let stride: usize = if colours.is_some() { 28 } else { 20 };
    let mut b = Builder { data: Vec::new() };
    let header = b.alloc(0xF0, 8);
    b.bytes(header, b"FRES    ");
    b.u32(header + 0x08, 0x000A_0400);
    b.u16(header + 0x0C, 0xFEFF);
    let file_name = b.string("tiny");
    b.u64(header + 0x20, file_name as u64);

    let model = b.alloc(0x78, 8);
    b.u64(header + 0x28, model as u64);
    b.u16(header + 0xDC, 1);
    b.bytes(model, b"FMDL");
    let model_name = b.string("Tiny");
    b.u64(model + 0x08, model_name as u64);

    // Skeleton.
    let skel = b.alloc(0x48, 8);
    b.u64(model + 0x18, skel as u64);
    b.bytes(skel, b"FSKL");
    let bones = b.alloc(2 * 0x58, 8);
    b.u64(skel + 0x10, bones as u64);
    let matrices = b.alloc(4, 8);
    b.u16(matrices, 0);
    b.u16(matrices + 2, 1);
    b.u64(skel + 0x18, matrices as u64);
    let inverse = b.alloc(2 * 48, 8);
    b.f32s(inverse, &[1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.]);
    b.f32s(
        inverse + 48,
        &[1., 0., 0., -1., 0., 1., 0., 0., 0., 0., 1., 0.],
    );
    b.u64(skel + 0x20, inverse as u64);
    b.u16(skel + 0x38, 2);
    b.u16(skel + 0x3A, 2);
    for (i, (name, parent, x)) in [("Root", -1i16, 0f32), ("Arm", 0, 1.)]
        .into_iter()
        .enumerate()
    {
        let at = bones + i * 0x58;
        let s = b.string(name);
        b.u64(at, s as u64);
        b.u16(at + 0x20, i as u16);
        b.i16(at + 0x22, parent);
        b.i16(at + 0x24, i as i16);
        b.i16(at + 0x26, -1);
        b.i16(at + 0x28, -1);
        b.f32s(at + 0x30, &[1., 1., 1., 0., 0., 0., 1., x, 0., 0.]);
    }

    // Vertex buffer: _p0 32_32_32 float, _u0 16_16 unorm, _i0 8_8 uint, _w0 8_8 unorm; stride 20 (and _c0
    // 16_16_16_16 float: stride 28).
    let vb = b.alloc(0x58, 8);
    b.u64(model + 0x20, vb as u64);
    b.u16(model + 0x68, 1);
    b.bytes(vb, b"FVTX");
    let mut attributes = vec![
        ("_p0", 0x1805u32, 0u16),
        ("_u0", 0x1201, 12),
        ("_i0", 0x0903, 16),
        ("_w0", 0x0901, 18),
    ];
    if colours.is_some() {
        attributes.push(("_c0", 0x1505, 20));
    }
    let attrs = b.alloc(attributes.len() * 0x10, 8);
    b.u64(vb + 0x08, attrs as u64);
    for (i, (name, format, offset)) in attributes.iter().copied().enumerate() {
        let at = attrs + i * 0x10;
        let s = b.string(name);
        b.u64(at, s as u64);
        b.u32(at + 8, format);
        b.u16(at + 0x0C, offset);
    }
    let sizes = b.alloc(16, 8);
    b.u32(sizes, 3 * stride as u32);
    b.u64(vb + 0x30, sizes as u64);
    let strides = b.alloc(16, 8);
    b.u32(strides, stride as u32);
    b.u64(vb + 0x38, strides as u64);
    b.u8(vb + 0x4C, attributes.len() as u8);
    b.u8(vb + 0x4D, 1);
    b.u32(vb + 0x50, 3);
    b.u16(vb + 0x54, 2);
    b.u16(vb + 0x56, 8);

    // Shape with one mesh (LOD 0).
    let shape = b.alloc(0x60, 8);
    b.u64(model + 0x28, shape as u64);
    b.u16(model + 0x6A, 1);
    b.bytes(shape, b"FSHP");
    b.u32(shape + 0x04, 0x2);
    let shape_name = b.string("Tri__Mt_Test");
    b.u64(shape + 0x08, shape_name as u64);
    let mesh = b.alloc(0x38, 8);
    b.u64(shape + 0x18, mesh as u64);
    b.u8(shape + 0x5A, 2);
    b.u8(shape + 0x5B, 1);
    let sub = b.alloc(8, 8);
    b.u32(sub + 4, 3);
    b.u64(mesh, sub as u64);
    let size = b.alloc(16, 8);
    b.u32(size, 6);
    b.u64(mesh + 0x18, size as u64);
    b.u32(mesh + 0x24, 3);
    b.u32(mesh + 0x28, 1);
    b.u32(mesh + 0x2C, 3);
    b.u16(mesh + 0x34, 1);

    // Material.
    let mat = b.alloc(0xB0, 8);
    b.u64(model + 0x38, mat as u64);
    b.u16(model + 0x6C, 1);
    b.bytes(mat, b"FMAT");
    let mat_name = b.string("Mt_Test");
    b.u64(mat + 0x08, mat_name as u64);
    let names = b.alloc(8, 8);
    let tex = b.string("Tex_Test_Alb");
    b.u64(names, tex as u64);
    b.u64(mat + 0x20, names as u64);
    let samplers = b.dict(&["_a0"]);
    b.u64(mat + 0x38, samplers as u64);
    b.u8(mat + 0xA2, 1);
    b.u8(mat + 0xA3, 1);

    // Bounds: skin bones Root and Arm, a box for the sub-mesh and one for the mesh, a sphere per skin bone
    // (values as the game's tools would compute them; see bfres_write).
    let skin_bones = b.alloc(4, 8);
    b.u16(skin_bones, 0);
    b.u16(skin_bones + 2, 1);
    b.u64(shape + 0x20, skin_bones as u64);
    b.u16(shape + 0x58, 2);
    let boxes = b.alloc(2 * 24, 8);
    for k in 0..2 {
        // Every vertex in the space of each of its slots' bones, empty slots too (matrix 0, Root): (0,0,0) and
        // (0,1,0) in Root's space; (1,0,0) in Arm's space (0,0,0) and Root's (1,0,0).
        b.f32s(boxes + k * 24, &[0.5, 0.5, 0.0, 0.5, 0.5, 0.0]);
    }
    b.u64(shape + 0x38, boxes as u64);
    let spheres = b.alloc(2 * 16, 8);
    b.f32s(spheres, &[0.0, 0.5, 0.0, 0.5001]);
    b.f32s(spheres + 16, &[0.0, 0.0, 0.0, 0.0001]);
    b.u64(shape + 0x40, spheres as u64);

    // GPU data as the game lays it out: index buffers (256-byte aligned), then vertex buffers, padded to 4 KiB;
    // then the memory pool block and the relocation table.
    let info = b.alloc(0x20, 8);
    b.u64(header + 0xB0, info as u64);
    let cpu_end = b.data.len();
    let gpu = b.alloc(0, 0x1000);
    b.u64(info + 8, gpu as u64);
    let idx = b.alloc(6, 0x100);
    for (i, v) in [0u16, 1, 2].into_iter().enumerate() {
        b.u16(idx + i * 2, v);
    }
    b.u32(mesh + 0x20, (idx - gpu) as u32);
    let vertex_start = b.alloc(0, 0x100);
    let verts = b.alloc(3 * stride, 0x100);
    b.u16(vb + 0x56, 0x100);
    let vertex = |p: [f32; 3], uv: [u16; 2], bone: u8, k: usize| {
        let mut v = Vec::new();
        p.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
        uv.iter()
            .for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
        v.extend_from_slice(&[bone, 0, 255, 0]);
        if let Some(c) = colours {
            c[k].iter()
                .for_each(|x| v.extend_from_slice(&f16_bits(*x).to_le_bytes()));
        }
        v
    };
    let mut vdata = vertex([0., 0., 0.], [0, 0], 0, 0);
    vdata.extend(vertex([1., 0., 0.], [65535, 0], 1, 1));
    vdata.extend(vertex([0., 1., 0.], [0, 65535], 0, 2));
    b.bytes(verts, &vdata);
    b.u32(vb + 0x48, (verts - gpu) as u32);
    let gpu_end = b.alloc(0, 0x1000);
    b.u32(info + 4, (gpu_end - gpu) as u32);
    let pool = b.alloc(0x120, 8);
    b.u64(header + 0xA8, pool as u64);

    let rlt = b.alloc(0x10 + 4 * 0x18 + 2 * 8, 8);
    b.bytes(rlt, b"_RLT");
    b.u32(rlt + 4, rlt as u32);
    b.u32(rlt + 8, 4);
    // (offset, size, first entry, entries): CPU part, index buffers, vertex buffers, memory pool.
    let sections = [
        (0, cpu_end, 0, 0),
        (gpu, vertex_start - gpu, 0, 1),
        (vertex_start, gpu_end - vertex_start, 1, 0),
        (pool, 0x120, 1, 1),
    ];
    for (i, (offset, size, first, count)) in sections.into_iter().enumerate() {
        let s = rlt + 0x10 + i * 0x18;
        b.u32(s + 8, offset as u32);
        b.u32(s + 12, size as u32);
        b.u32(s + 16, first);
        b.u32(s + 20, count);
    }
    // The pointers into the GPU data (the buffer info's) and into the memory pool (the header's).
    let entries = rlt + 0x10 + 4 * 0x18;
    for (k, at) in [info + 8, header + 0xA8].into_iter().enumerate() {
        b.u32(entries + k * 8, at as u32);
        b.u16(entries + k * 8 + 4, 1);
        b.u8(entries + k * 8 + 6, 1);
    }
    b.u32(header + 0x18, rlt as u32);
    let len = b.data.len() as u32;
    b.u32(header + 0x1C, len);
    b.data
}

/// A BFRES 10.4 animation file (`.anim.bfres`) with one skeletal animation `Wave` (Euler, looping, 10 frames) of
/// two bones, `Root` and `Arm`, each with all three base values and no curves: `Root` at rest, `Arm` at x = 1 turned
/// 0.5 radians about z. Its relocation table has one section, listing its pointers as the game's files do (a bone
/// without curves has its curve pointer null and unlisted; the two unused pointer words after the base values are
/// listed).
pub fn tiny_anim() -> Vec<u8> {
    let mut b = Builder { data: Vec::new() };
    let header = b.alloc(0xF0, 8);
    b.bytes(header, b"FRES    ");
    b.u32(header + 0x08, 0x000A_0400);
    b.u16(header + 0x0C, 0xFEFF);
    let mut listed: Vec<usize> = Vec::new();
    let anim = b.alloc(0x50, 8);
    b.u64(header + 0x58, anim as u64);
    b.u16(header + 0xE2, 1);
    listed.extend([header + 0x20, header + 0x58]);
    b.bytes(anim, b"FSKA");
    // Euler, looping
    b.u32(anim + 0x04, 1 << 12 | 1 << 2);
    let bind = b.alloc(4, 8);
    b.u16(bind, 0xFFFF);
    b.u16(bind + 2, 0xFFFF);
    b.u64(anim + 0x20, bind as u64);
    let bones = b.alloc(2 * 0x38, 8);
    b.u64(anim + 0x28, bones as u64);
    listed.extend([anim + 0x08, anim + 0x10, anim + 0x20, anim + 0x28]);
    b.u32(anim + 0x40, 10);
    b.u16(anim + 0x4C, 2);
    let rest: [([f32; 3], [f32; 4], [f32; 3]); 2] = [
        ([1.0; 3], [0.0, 0.0, 0.0, 1.0], [0.0; 3]),
        ([1.0; 3], [0.0, 0.0, 0.5, 1.0], [1.0, 0.0, 0.0]),
    ];
    let mut base_blocks = Vec::new();
    for (s, r, tr) in rest {
        let at = b.alloc(40, 8);
        b.f32s(at, &s);
        b.f32s(at + 12, &r);
        b.f32s(at + 28, &tr);
        base_blocks.push(at);
    }
    for (i, base) in base_blocks.into_iter().enumerate() {
        let at = bones + i * 0x38;
        b.u64(at + 0x10, base as u64);
        // base scale, rotation and translation; bit 23; scale uniform and one; rotation zero for Root only;
        // translation zero for Root only
        let zero = if i == 0 { 0x0C00_0000 } else { 0 };
        b.u32(at + 0x28, 0x38 | 1 << 23 | 0x0300_0000 | zero);
        b.bytes(at + 0x2C, &[3, 6, 0, 7]);
        listed.extend([at, at + 0x10, at + 0x18, at + 0x20]);
    }
    // the strings
    let file_name = b.string("tiny_anim");
    b.u64(header + 0x20, file_name as u64);
    let name = b.string("Wave");
    b.u64(anim + 0x08, name as u64);
    let path = b.string("");
    b.u64(anim + 0x10, path as u64);
    for (i, n) in ["Root", "Arm"].into_iter().enumerate() {
        let s = b.string(n);
        b.u64(bones + i * 0x38, s as u64);
    }
    let end = b.data.len();
    listed.sort();
    let rlt = b.alloc(0x10 + 0x18 + listed.len() * 8, 8);
    b.bytes(rlt, b"_RLT");
    b.u32(rlt + 4, rlt as u32);
    b.u32(rlt + 8, 1);
    b.u32(rlt + 0x10 + 8, 0);
    b.u32(rlt + 0x10 + 12, end as u32);
    b.u32(rlt + 0x10 + 16, 0);
    b.u32(rlt + 0x10 + 20, listed.len() as u32);
    let entries = rlt + 0x10 + 0x18;
    for (k, at) in listed.into_iter().enumerate() {
        b.u32(entries + k * 8, at as u32);
        b.u16(entries + k * 8 + 4, 1);
        b.u8(entries + k * 8 + 6, 1);
    }
    b.u32(header + 0x18, rlt as u32);
    let len = b.data.len() as u32;
    b.u32(header + 0x1C, len);
    b.data
}

/// A BNTX with one `width` x `height` RGBA8 texture called `name` laid out for `gob`: pixel (x, y) is
/// `[x, y, x ^ y, 255]` (as bytes).
pub fn tiny_bntx(name: &str, width: u32, height: u32, gob: Gob) -> Vec<u8> {
    let surface = Surface::mip(gob, width, height, 0, (1, 1), 4, 0);
    let linear: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, (x ^ y) as u8, 255]))
        .collect();
    let swizzled = surface.swizzle(&linear).expect("sized");

    let mut b = Builder { data: Vec::new() };
    let header = b.alloc(0x40, 8);
    b.bytes(header, b"BNTX");
    b.u16(header + 0x0C, 0xFEFF);
    b.bytes(
        header + 0x20,
        if gob == Gob::Orin { b"Ounc" } else { b"NX  " },
    );
    b.u32(header + 0x24, 1);
    let array = b.alloc(8, 8);
    b.u64(header + 0x28, array as u64);
    let brti = b.alloc(0xA0, 8);
    b.u64(array, brti as u64);
    b.bytes(brti, b"BRTI");
    b.u8(brti + 0x11, 2);
    b.u16(brti + 0x16, 1);
    b.u32(brti + 0x18, 1);
    b.u32(brti + 0x1C, 0x0b01);
    b.u32(brti + 0x24, width);
    b.u32(brti + 0x28, height);
    b.u32(brti + 0x2C, 1);
    b.u32(brti + 0x30, 1);
    b.u32(brti + 0x50, swizzled.len() as u32);
    b.u32(brti + 0x58, 0x0504_0302);
    b.u32(brti + 0x5C, 1);
    let s = b.string(name);
    b.u64(brti + 0x60, s as u64);
    let mips = b.alloc(8, 8);
    b.u64(brti + 0x70, mips as u64);
    let cpu_end = b.data.len();
    // The image data block (BRTD), its data 0x200-aligned right after its 16-byte header, as in the game.
    let brtd = b.alloc(0, 0x200) + 0x200 - 0x10;
    b.data.resize(brtd + 0x10, 0);
    b.bytes(brtd, b"BRTD");
    b.u64(header + 0x30, brtd as u64);
    let data = b.alloc(swizzled.len(), 0x10);
    b.bytes(data, &swizzled);
    b.u64(mips, data as u64);
    // The relocation table on the next 4 KiB: the CPU part, and the data block with the pointers into it (the
    // header's BRTD pointer and the mip pointer).
    let rlt = b.alloc(0, 0x1000);
    b.u64(brtd + 8, (rlt - brtd) as u64);
    let rlt = b.alloc(0x10 + 2 * 0x18 + 2 * 8, 8);
    b.bytes(rlt, b"_RLT");
    b.u32(rlt + 4, rlt as u32);
    b.u32(rlt + 8, 2);
    for (i, (offset, size, first, count)) in [(0, cpu_end, 0, 0), (brtd, rlt - brtd, 0, 2)]
        .into_iter()
        .enumerate()
    {
        let sec = rlt + 0x10 + i * 0x18;
        b.u32(sec + 8, offset as u32);
        b.u32(sec + 12, size as u32);
        b.u32(sec + 16, first);
        b.u32(sec + 20, count);
    }
    let entries = rlt + 0x10 + 2 * 0x18;
    for (k, at) in [header + 0x30, mips].into_iter().enumerate() {
        b.u32(entries + k * 8, at as u32);
        b.u16(entries + k * 8 + 4, 1);
        b.u8(entries + k * 8 + 6, 1);
    }
    b.u32(header + 0x18, rlt as u32);
    let len = b.data.len() as u32;
    b.u32(header + 0x1C, len);
    b.data
}

/// A square grid of `n` x `n` cells (2 triangles each) in the xz plane, `size` across, from `at`: normals up,
/// tangents along x, UV set 0 across the grid (0..1) and set 1 at `uv1_scale` times it, colour set 0 constant in
/// channel 0, varying in 1 and 3 (in 0..1, or up to `colour_top` in channel 3).
pub fn meshlet_grid(
    n: usize,
    size: f32,
    at: [f32; 3],
    uv1_scale: f32,
    colour_top: f32,
) -> crate::meshlet::MeshletGeometry {
    let mut g = crate::meshlet::MeshletGeometry {
        tangents: Some(Vec::new()),
        uvs: vec![Vec::new(), Vec::new()],
        colours: vec![(0, Vec::new())],
        ..Default::default()
    };
    for j in 0..=n {
        for i in 0..=n {
            let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
            g.positions.push([
                at[0] + u * size,
                at[1] + (u * 7.0).sin() * 0.01,
                at[2] + v * size,
            ]);
            g.normals.push([0.0, 1.0, 0.0]);
            g.tangents
                .as_mut()
                .unwrap()
                .push([1.0, 0.0, 0.0, if i % 2 == 0 { 1.0 } else { -1.0 }]);
            g.uvs[0].push([u, v]);
            g.uvs[1].push([u * uv1_scale, -v * uv1_scale]);
            g.colours[0].1.push([1.0, u, 0.5, v * colour_top]);
        }
    }
    let w = n as u32 + 1;
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * w + i;
            g.indices.extend([a, a + w, a + 1, a + 1, a + w, a + w + 1]);
        }
    }
    g
}

/// The buffer flags of [`meshlet_grid`]'s shape: positions, normals, tangents, two UV sets, colour set 0.
pub const MESHLET_GRID_FLAGS: u32 =
    0x1061 | crate::meshlet::flags::UV_1 | crate::meshlet::flags::COLOUR_0;

/// A BFRES 10.4 file laid out as the game's map pieces and objects are: one model of one bone `Root` and one meshlet
/// shape `Rock__Mt_Test` (unskinned, an empty vertex buffer, no meshes), whose buffer holds `lods`
/// ([`crate::meshlet::encode`] with [`MESHLET_GRID_FLAGS`]); its record, box and sphere as the game's. The GPU data
/// (the relocation table's section 3; sections 1 and 2 empty at the data's end) on the 4 KiB after the data, the
/// memory pool block after it, pointed at by the header and the shape's meshlet record.
pub fn tiny_meshlet_bfres(lods: &[crate::meshlet::MeshletGeometry]) -> Vec<u8> {
    let encoded =
        crate::meshlet::encode(MESHLET_GRID_FLAGS, crate::meshlet::STEP, lods).expect("encode");
    let mut b = Builder { data: Vec::new() };
    let header = b.alloc(0xF0, 8);
    b.bytes(header, b"FRES    ");
    b.u32(header + 0x08, 0x000A_0400);
    b.u16(header + 0x0C, 0xFEFF);
    let file_name = b.string("rock");
    b.u64(header + 0x20, file_name as u64);

    let model = b.alloc(0x78, 8);
    b.u64(header + 0x28, model as u64);
    b.u16(header + 0xDC, 1);
    b.bytes(model, b"FMDL");
    let model_name = b.string("Rock");
    b.u64(model + 0x08, model_name as u64);

    // Skeleton: one bone.
    let skel = b.alloc(0x48, 8);
    b.u64(model + 0x18, skel as u64);
    b.bytes(skel, b"FSKL");
    let bones = b.alloc(0x58, 8);
    b.u64(skel + 0x10, bones as u64);
    let matrices = b.alloc(2, 8);
    b.u64(skel + 0x18, matrices as u64);
    let inverse = b.alloc(48, 8);
    b.f32s(inverse, &[1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.]);
    b.u64(skel + 0x20, inverse as u64);
    b.u16(skel + 0x38, 1);
    b.u16(skel + 0x3A, 1);
    let s = b.string("Root");
    b.u64(bones, s as u64);
    b.i16(bones + 0x22, -1);
    b.i16(bones + 0x24, 0);
    b.i16(bones + 0x26, -1);
    b.i16(bones + 0x28, -1);
    b.f32s(bones + 0x30, &[1., 1., 1., 0., 0., 0., 1., 0., 0., 0.]);

    // An empty vertex buffer slot, as meshlet shapes have.
    let vb = b.alloc(0x58, 8);
    b.u64(model + 0x20, vb as u64);
    b.u16(model + 0x68, 1);

    // The shape and its meshlet record.
    let shape = b.alloc(0x60, 8);
    b.u64(model + 0x28, shape as u64);
    b.u16(model + 0x6A, 1);
    b.bytes(shape, b"FSHP");
    b.u32(shape + 0x04, 0x12);
    let shape_name = b.string("Rock__Mt_Test");
    b.u64(shape + 0x08, shape_name as u64);
    let record = b.alloc(0x48, 8);
    b.u64(shape + 0x18, record as u64);
    let blank = b.alloc(0x48, 8);
    b.u64(record + 0x08, blank as u64);
    let infos = b.alloc(0x10, 8);
    b.u32(infos, encoded.data.len() as u32);
    b.u32(infos + 4, 0x40);
    b.u64(record + 0x10, infos as u64);
    let offsets = b.alloc(8, 8);
    b.u64(record + 0x18, offsets as u64);
    let tris = b.alloc(4 * lods.len(), 8);
    let counts = b.alloc(4 * lods.len(), 8);
    for k in 0..lods.len() {
        b.u32(tris + 4 * k, encoded.lod_triangles[k]);
        b.u32(counts + 4 * k, encoded.lod_meshlets[k]);
    }
    b.u64(record + 0x20, tris as u64);
    b.u64(record + 0x28, counts as u64);
    b.u32(record + 0x3C, 64);
    b.u32(record + 0x40, 124);
    b.u8(record + 0x44, 1);
    b.u8(record + 0x45, lods.len() as u8);
    let bounds = b.alloc(24 + 16, 8);
    b.u64(shape + 0x38, bounds as u64);
    b.u64(shape + 0x40, (bounds + 24) as u64);
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &encoded.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let centre: Vec<f32> = (0..3).map(|k| (lo[k] + hi[k]) / 2.0).collect();
    let extent: Vec<f32> = (0..3).map(|k| (hi[k] - lo[k]) / 2.0).collect();
    b.f32s(bounds, &centre);
    b.f32s(bounds + 12, &extent);
    let r = (extent.iter().map(|e| e * e).sum::<f32>()).sqrt();
    b.f32s(bounds + 24, &[centre[0], centre[1], centre[2], r]);

    // Material.
    let mat = b.alloc(0xB0, 8);
    b.u64(model + 0x38, mat as u64);
    b.u16(model + 0x6C, 1);
    b.bytes(mat, b"FMAT");
    let mat_name = b.string("Mt_Test");
    b.u64(mat + 0x08, mat_name as u64);

    // GPU data: the buffer at 0, padded to 4 KiB; then the memory pool block and the table.
    let info = b.alloc(0x20, 8);
    b.u32(info, 0x24);
    b.u64(header + 0xB0, info as u64);
    let cpu_end = b.data.len();
    let gpu = b.alloc(0, 0x1000);
    b.u64(info + 8, gpu as u64);
    let buffer = b.alloc(encoded.data.len(), 0x40);
    b.bytes(buffer, &encoded.data);
    let gpu_end = b.alloc(0, 0x1000);
    b.u32(info + 4, (gpu_end - gpu) as u32);
    let pool = b.alloc(0x120, 8);
    b.u64(header + 0xA8, pool as u64);
    b.u64(record, pool as u64);

    let rlt = b.alloc(0x10 + 6 * 0x18 + 3 * 8, 8);
    b.bytes(rlt, b"_RLT");
    b.u32(rlt + 4, rlt as u32);
    b.u32(rlt + 8, 6);
    // (offset, size, first entry, entries): the data, index and vertex buffers (none), the GPU data, the memory
    // pool, the table.
    let sections = [
        (0, cpu_end, 0, 0),
        (cpu_end, 0, 0, 0),
        (cpu_end, 0, 0, 0),
        (gpu, gpu_end - gpu, 0, 1),
        (pool, 0x120, 1, 2),
        (rlt, 0, 3, 0),
    ];
    for (i, (offset, size, first, count)) in sections.into_iter().enumerate() {
        let s = rlt + 0x10 + i * 0x18;
        b.u32(s + 8, offset as u32);
        b.u32(s + 12, size as u32);
        b.u32(s + 16, first);
        b.u32(s + 20, count);
    }
    // The pointer into the GPU data (the buffer info's) and those into the memory pool (the header's, the record's).
    let entries = rlt + 0x10 + 6 * 0x18;
    for (k, at) in [info + 8, header + 0xA8, record].into_iter().enumerate() {
        b.u32(entries + k * 8, at as u32);
        b.u16(entries + k * 8 + 4, 1);
        b.u8(entries + k * 8 + 6, 1);
    }
    b.u32(header + 0x18, rlt as u32);
    let len = b.data.len() as u32;
    b.u32(header + 0x1C, len);
    b.data
}
