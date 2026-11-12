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

    // Vertex buffer: _p0 32_32_32 float, _u0 16_16 unorm, _i0 8_8 uint, _w0 8_8 unorm; stride 20.
    let vb = b.alloc(0x58, 8);
    b.u64(model + 0x20, vb as u64);
    b.u16(model + 0x68, 1);
    b.bytes(vb, b"FVTX");
    let attrs = b.alloc(4 * 0x10, 8);
    b.u64(vb + 0x08, attrs as u64);
    for (i, (name, format, offset)) in [
        ("_p0", 0x1805u32, 0u16),
        ("_u0", 0x1201, 12),
        ("_i0", 0x0903, 16),
        ("_w0", 0x0901, 18),
    ]
    .into_iter()
    .enumerate()
    {
        let at = attrs + i * 0x10;
        let s = b.string(name);
        b.u64(at, s as u64);
        b.u32(at + 8, format);
        b.u16(at + 0x0C, offset);
    }
    let sizes = b.alloc(16, 8);
    b.u32(sizes, 60);
    b.u64(vb + 0x30, sizes as u64);
    let strides = b.alloc(16, 8);
    b.u32(strides, 20);
    b.u64(vb + 0x38, strides as u64);
    b.u8(vb + 0x4C, 4);
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
    let verts = b.alloc(60, 0x100);
    b.u16(vb + 0x56, 0x100);
    let vertex = |p: [f32; 3], uv: [u16; 2], bone: u8| {
        let mut v = Vec::new();
        p.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
        uv.iter()
            .for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
        v.extend_from_slice(&[bone, 0, 255, 0]);
        v
    };
    let mut vdata = vertex([0., 0., 0.], [0, 0], 0);
    vdata.extend(vertex([1., 0., 0.], [65535, 0], 1));
    vdata.extend(vertex([0., 1., 0.], [0, 65535], 0));
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
