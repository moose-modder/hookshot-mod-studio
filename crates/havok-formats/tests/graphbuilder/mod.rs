//! A tagfile builder for synthetic object graphs: classes described by their members' types, objects given as values,
//! and the layout (member offsets, item table, DATA) computed. A member a value leaves out is stored as zero bytes (a
//! zero number, an empty array or string, a null pointer). Classes are flat: every member, inherited ones included, is
//! declared on the class itself, which reads the same through the value view. No game payload is embedded.
#![allow(dead_code, reason = "each test file uses part of the builder")]

use std::collections::BTreeMap;

/// A member's type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ty {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    F32,
    Bool,
    /// `hkStringPtr`.
    Str,
    /// A pointer to an object of the class.
    Ptr(&'static str),
    /// `hkArray` of the element type.
    Arr(Box<Ty>),
    /// A fixed-size array (`T[N]`).
    Fixed(Box<Ty>, u32),
    /// A record of the class, stored inline.
    Class(&'static str),
}

pub fn arr(t: Ty) -> Ty {
    Ty::Arr(Box::new(t))
}

pub fn fixed(t: Ty, n: u32) -> Ty {
    Ty::Fixed(Box::new(t), n)
}

/// `hkVector4`.
pub fn vec4() -> Ty {
    fixed(Ty::F32, 4)
}

/// `hkMatrix4` / `hkTransform`.
pub fn mat4() -> Ty {
    fixed(Ty::F32, 16)
}

/// A value.
#[derive(Clone, Debug)]
pub enum V {
    I(i64),
    F(f32),
    S(String),
    /// A pointer to object `n` of the graph (0 is the root).
    Obj(usize),
    Null,
    /// Array elements (stored or fixed-size).
    A(Vec<V>),
    /// A record's members by name.
    R(Vec<(&'static str, V)>),
}

pub fn r(fields: Vec<(&'static str, V)>) -> V {
    V::R(fields)
}

pub fn s(text: &str) -> V {
    V::S(text.to_owned())
}

pub fn floats(values: &[f32]) -> V {
    V::A(values.iter().map(|&x| V::F(x)).collect())
}

pub fn ints(values: &[i64]) -> V {
    V::A(values.iter().map(|&x| V::I(x)).collect())
}

/// Classes by name: their members in order.
pub type Schema = BTreeMap<&'static str, Vec<(&'static str, Ty)>>;

/// The objects of a graph: class and value. Object 0 is the root.
pub type Graph = Vec<(&'static str, V)>;

fn size_align(schema: &Schema, t: &Ty) -> (usize, usize) {
    match t {
        Ty::U8 | Ty::I8 | Ty::Bool => (1, 1),
        Ty::U16 | Ty::I16 => (2, 2),
        Ty::U32 | Ty::I32 | Ty::F32 => (4, 4),
        Ty::U64 | Ty::Str | Ty::Ptr(_) => (8, 8),
        Ty::Arr(_) => (16, 8),
        Ty::Fixed(e, n) => {
            let (size, align) = size_align(schema, e);
            (size * *n as usize, align)
        }
        Ty::Class(c) => class_layout(schema, c).1,
    }
}

/// A class's member offsets, then its size and alignment.
fn class_layout(schema: &Schema, class: &str) -> (Vec<usize>, (usize, usize)) {
    let members = schema
        .get(class)
        .unwrap_or_else(|| panic!("no class {class} in the schema"));
    let mut offset: usize = 0;
    let mut align: usize = 1;
    let mut offsets = Vec::new();
    for (_, t) in members {
        let (size, a) = size_align(schema, t);
        offset = offset.next_multiple_of(a);
        offsets.push(offset);
        offset += size;
        align = align.max(a);
    }
    (offsets, (offset.next_multiple_of(align).max(1), align))
}

/// One type of the table: name, format word, size, subtype, and a class's members (name, offset, type index).
struct TypeDef {
    name: String,
    format: u32,
    size: u32,
    subtype: Option<u32>,
    fields: Vec<(&'static str, u32, u32)>,
}

struct Types<'s> {
    schema: &'s Schema,
    /// A type's key (its Debug text) -> index, and the definitions in index order.
    index: BTreeMap<String, u32>,
    defs: Vec<TypeDef>,
}

impl Types<'_> {
    fn of(&mut self, t: &Ty) -> u32 {
        let key = format!("{t:?}");
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let (size, _) = size_align(self.schema, t);
        let size = size as u32;
        let (name, format, subtype): (String, u32, Option<u32>) = match t {
            Ty::U8 => ("hkUint8".into(), 0x2004, None),
            Ty::I8 => ("hkInt8".into(), 0x2204, None),
            Ty::U16 => ("hkUint16".into(), 0x4004, None),
            Ty::I16 => ("hkInt16".into(), 0x4204, None),
            Ty::U32 => ("hkUint32".into(), 0x8004, None),
            Ty::I32 => ("hkInt32".into(), 0x8204, None),
            Ty::U64 => ("hkUint64".into(), 0x10004, None),
            Ty::F32 => ("hkReal".into(), 0x174605, None),
            Ty::Bool => ("hkBool".into(), 0x2002, None),
            Ty::Str => ("hkStringPtr".into(), 3, None),
            Ty::Ptr(c) => {
                let target = self.of(&Ty::Class(c));
                ("T*".into(), 6, Some(target))
            }
            Ty::Arr(e) => {
                let element = self.of(e);
                ("hkArray".into(), 8, Some(element))
            }
            Ty::Fixed(e, n) => {
                let element = self.of(e);
                ("T[N]".into(), 8 | 32 | (n << 8), Some(element))
            }
            Ty::Class(c) => {
                // reserve the index first: a class may point to itself
                let i = self.reserve(&key, c.to_string(), 7, size, None);
                let (offsets, _) = class_layout(self.schema, c);
                let members = self.schema[c].clone();
                let fields = members
                    .iter()
                    .zip(offsets)
                    .map(|((name, t), o)| (*name, o as u32, self.of(t)))
                    .collect();
                self.defs[i as usize - 1].fields = fields;
                return i;
            }
        };
        self.reserve(&key, name, format, size, subtype)
    }
    fn reserve(&mut self, key: &str, name: String, format: u32, size: u32, subtype: Option<u32>) -> u32 {
        self.defs.push(TypeDef {
            name,
            format,
            size,
            subtype,
            fields: Vec::new(),
        });
        let i = self.defs.len() as u32;
        self.index.insert(key.to_owned(), i);
        i
    }
}

struct Writer<'s> {
    schema: &'s Schema,
    types: Types<'s>,
    data: Vec<u8>,
    /// type index, DATA offset, count, is a run (array or string)
    items: Vec<(u32, u32, u32, bool)>,
}

impl Writer<'_> {
    fn alloc(&mut self, type_index: u32, size: usize, count: usize, run: bool) -> (u32, usize) {
        while !self.data.len().is_multiple_of(16) {
            self.data.push(0);
        }
        let offset = self.data.len();
        self.data.resize(offset + size * count, 0);
        self.items.push((type_index, offset as u32, count as u32, run));
        (self.items.len() as u32, offset)
    }
    fn put(&mut self, at: usize, bytes: &[u8]) {
        self.data[at..at + bytes.len()].copy_from_slice(bytes);
    }
    /// Writes value `v` of type `t` at DATA offset `at`; object pointers are item `first_object + n`.
    fn write(&mut self, t: &Ty, v: &V, at: usize, first_object: u32) {
        match (t, v) {
            (Ty::U8 | Ty::I8 | Ty::Bool, V::I(x)) => self.put(at, &(*x as u8).to_le_bytes()),
            (Ty::U16 | Ty::I16, V::I(x)) => self.put(at, &(*x as u16).to_le_bytes()),
            (Ty::U32 | Ty::I32, V::I(x)) => self.put(at, &(*x as u32).to_le_bytes()),
            (Ty::U64, V::I(x)) => self.put(at, &(*x as u64).to_le_bytes()),
            (Ty::F32, V::F(x)) => self.put(at, &x.to_le_bytes()),
            (Ty::Str, V::S(text)) => {
                let char_type = self.types.of(&Ty::U8);
                let mut bytes = text.as_bytes().to_vec();
                bytes.push(0);
                let (item, offset) = self.alloc(char_type, 1, bytes.len(), true);
                self.put(offset, &bytes);
                self.put(at, &u64::from(item).to_le_bytes());
            }
            (Ty::Ptr(_), V::Obj(n)) => {
                let item = first_object + *n as u32;
                self.put(at, &u64::from(item).to_le_bytes());
            }
            (Ty::Ptr(_) | Ty::Str, V::Null) => {}
            (Ty::Arr(e), V::A(elements)) => {
                if elements.is_empty() {
                    return;
                }
                let element_type = self.types.of(e);
                let (size, _) = size_align(self.schema, e);
                let (item, offset) = self.alloc(element_type, size, elements.len(), true);
                for (i, x) in elements.iter().enumerate() {
                    self.write(e, x, offset + i * size, first_object);
                }
                self.put(at, &item.to_le_bytes());
            }
            (Ty::Fixed(e, n), V::A(elements)) => {
                assert!(
                    elements.len() <= *n as usize,
                    "{} elements for a fixed array of {n}",
                    elements.len()
                );
                let (size, _) = size_align(self.schema, e);
                for (i, x) in elements.iter().enumerate() {
                    self.write(e, x, at + i * size, first_object);
                }
            }
            (Ty::Class(c), V::R(fields)) => {
                let (offsets, _) = class_layout(self.schema, c);
                let members = &self.schema[c];
                for (name, x) in fields {
                    let i = members
                        .iter()
                        .position(|(m, _)| m == name)
                        .unwrap_or_else(|| panic!("class {c} has no member {name}"));
                    let member_type = members[i].1.clone();
                    self.write(&member_type, x, at + offsets[i], first_object);
                }
            }
            (t, v) => panic!("value {v:?} for a {t:?}"),
        }
    }
}

fn section(name: &[u8; 4], leaf: bool, bytes: Vec<u8>) -> Vec<u8> {
    let mut out = ((bytes.len() + 8) as u32 | if leaf { 0x4000_0000 } else { 0 })
        .to_be_bytes()
        .to_vec();
    out.extend(name);
    out.extend(bytes);
    out
}

fn packed(value: u32) -> Vec<u8> {
    if value < 128 {
        vec![value as u8]
    } else if value < 16384 {
        (value as u16 | 0x8000).to_be_bytes().to_vec()
    } else {
        vec![0xc0 | (value >> 16) as u8, (value >> 8) as u8, value as u8]
    }
}

/// The tagfile of `sdk` holding `graph`'s objects, laid out by `schema`.
pub fn build(sdk: &[u8; 8], schema: &Schema, graph: &Graph) -> Vec<u8> {
    let mut w = Writer {
        schema,
        types: Types {
            schema,
            index: BTreeMap::new(),
            defs: Vec::new(),
        },
        data: Vec::new(),
        items: Vec::new(),
    };
    // the objects come first, so a pointer to object n is item n + 1
    let mut placed = Vec::new();
    for (class, _) in graph {
        let t = w.types.of(&Ty::Class(class));
        let (size, _) = size_align(schema, &Ty::Class(class));
        placed.push(w.alloc(t, size, 1, false).1);
    }
    for ((class, value), at) in graph.iter().zip(placed) {
        w.write(&Ty::Class(class), value, at, 1);
    }
    let mut names = Vec::new();
    let mut field_names = Vec::new();
    let mut type_names = packed(w.types.defs.len() as u32 + 1);
    let mut bodies = Vec::new();
    let mut field_index = 0;
    for (
        index,
        TypeDef {
            name,
            format,
            size,
            subtype,
            fields,
        },
    ) in w.types.defs.iter().enumerate()
    {
        names.extend(name.bytes());
        names.push(0);
        type_names.extend(packed(index as u32));
        type_names.push(0);
        bodies.extend(packed(index as u32 + 1));
        bodies.push(0); // no parent
        let is_record = *format == 7;
        bodies.extend(packed(
            9 | if subtype.is_some() { 2 } else { 0 } | if is_record { 32 } else { 0 },
        ));
        bodies.extend(packed(*format));
        if let Some(subtype) = subtype {
            bodies.extend(packed(*subtype));
        }
        bodies.extend(packed(*size));
        bodies.push(4);
        if is_record {
            bodies.extend(packed(fields.len() as u32));
            for &(name, offset, kind) in fields {
                field_names.extend(name.bytes());
                field_names.push(0);
                bodies.extend(packed(field_index));
                field_index += 1;
                bodies.push(32);
                bodies.extend(packed(offset));
                bodies.extend(packed(kind));
            }
        }
    }
    let mut types = section(b"TSTR", true, names);
    types.extend(section(b"TNA1", true, type_names));
    types.extend(section(b"FSTR", true, field_names));
    types.extend(section(b"TBDY", true, bodies));
    let mut table = vec![0; 12];
    for &(type_index, offset, count, run) in &w.items {
        table.extend((type_index | if run { 0x2000_0000 } else { 0x1000_0000 }).to_le_bytes());
        table.extend(offset.to_le_bytes());
        table.extend(count.to_le_bytes());
    }
    let mut root = section(b"SDKV", true, sdk.to_vec());
    root.extend(section(b"DATA", true, w.data));
    root.extend(section(b"TYPE", false, types));
    root.extend(section(b"INDX", false, section(b"ITEM", true, table)));
    section(b"TAG0", false, root)
}
