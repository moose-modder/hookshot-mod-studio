//! A miniature tagfile builder for the synthetic tests: a type table from a list of definitions, the DATA bytes the
//! test lays out, and the item table. Type indices are one-based, in definition order. No game payload is embedded.
#![allow(dead_code, reason = "each test file uses part of the builder")]

/// A size or id that does not fit its field is the fixture's mistake: it comes back as the conversion error.
pub type Built<T> = Result<T, std::num::TryFromIntError>;

/// One type: a primitive, pointer or array (`fields` empty) or a class (format 7).
pub struct Definition {
    pub name: &'static str,
    pub format: u32,
    pub size: u32,
    pub subtype: Option<u32>,
    /// Name, offset, type index.
    pub fields: Vec<(&'static str, u32, u32)>,
}

pub fn primitive(name: &'static str, format: u32, size: u32, subtype: Option<u32>) -> Definition {
    Definition {
        name,
        format,
        size,
        subtype,
        fields: Vec::new(),
    }
}

/// A fixed-size array of `count` elements of type `element` (of `element_size` bytes).
pub fn fixed(name: &'static str, count: u32, element: u32, element_size: u32) -> Definition {
    primitive(name, 8 | 32 | (count << 8), count * element_size, Some(element))
}

pub fn record(name: &'static str, size: u32, fields: Vec<(&'static str, u32, u32)>) -> Definition {
    Definition {
        name,
        format: 7,
        size,
        subtype: None,
        fields,
    }
}

/// One entry of the item table: `count` elements of a type at a DATA offset, as an array item or an object item.
pub struct Item {
    pub type_index: u32,
    pub offset: u32,
    pub count: u32,
    pub array: bool,
}

fn section(name: &[u8; 4], leaf: bool, bytes: Vec<u8>) -> Built<Vec<u8>> {
    let mut out = (u32::try_from(bytes.len() + 8)? | if leaf { 0x4000_0000 } else { 0 })
        .to_be_bytes()
        .to_vec();
    out.extend(name);
    out.extend(bytes);
    Ok(out)
}

fn packed(value: u32) -> Built<Vec<u8>> {
    Ok(if value < 128 {
        vec![u8::try_from(value)?]
    } else if value < 16384 {
        (u16::try_from(value)? | 0x8000).to_be_bytes().to_vec()
    } else {
        vec![0xc0 | u8::try_from(value >> 16)?, (value >> 8) as u8, value as u8]
    })
}

/// The tagfile of `sdk` (`b"20160200"` or `b"20180100"`).
pub fn tagfile(sdk: &[u8; 8], definitions: &[Definition], data: Vec<u8>, items: &[Item]) -> Built<Vec<u8>> {
    let mut names = Vec::new();
    let mut field_names = Vec::new();
    let mut type_names = packed(u32::try_from(definitions.len() + 1)?)?;
    let mut bodies = Vec::new();
    let mut field_index = 0;
    for (index, definition) in definitions.iter().enumerate() {
        names.extend(definition.name.bytes());
        names.push(0);
        type_names.extend(packed(u32::try_from(index)?)?);
        type_names.push(0);
        bodies.extend(packed(u32::try_from(index + 1)?)?);
        bodies.push(0); // no parent
        let is_record = definition.format == 7;
        bodies.extend(packed(
            9 | if definition.subtype.is_some() { 2 } else { 0 } | if is_record { 32 } else { 0 },
        )?);
        bodies.extend(packed(definition.format)?);
        if let Some(subtype) = definition.subtype {
            bodies.extend(packed(subtype)?);
        }
        bodies.extend(packed(definition.size)?);
        bodies.push(4);
        if is_record {
            bodies.extend(packed(u32::try_from(definition.fields.len())?)?);
            for &(name, offset, kind) in &definition.fields {
                field_names.extend(name.bytes());
                field_names.push(0);
                bodies.extend(packed(field_index)?);
                field_index += 1;
                bodies.push(32);
                bodies.extend(packed(offset)?);
                bodies.extend(packed(kind)?);
            }
        }
    }
    let mut types = section(b"TSTR", true, names)?;
    types.extend(section(b"TNA1", true, type_names)?);
    types.extend(section(b"FSTR", true, field_names)?);
    types.extend(section(b"TBDY", true, bodies)?);
    let mut table = vec![0; 12];
    for item in items {
        table.extend((item.type_index | if item.array { 0x2000_0000 } else { 0x1000_0000 }).to_le_bytes());
        table.extend(item.offset.to_le_bytes());
        table.extend(item.count.to_le_bytes());
    }
    let mut root = section(b"SDKV", true, sdk.to_vec())?;
    root.extend(section(b"DATA", true, data)?);
    root.extend(section(b"TYPE", false, types)?);
    root.extend(section(b"INDX", false, section(b"ITEM", true, table)?)?);
    section(b"TAG0", false, root)
}

/// Lays objects and arrays out in DATA, 16-aligned, and numbers the items.
#[derive(Default)]
pub struct Layout {
    pub data: Vec<u8>,
    pub items: Vec<Item>,
}

impl Layout {
    /// Reserve `size` zero bytes for `count` elements of `type_index`; the item's index (items start at 1) and its
    /// DATA offset.
    pub fn alloc(&mut self, type_index: u32, size: usize, count: u32, array: bool) -> Built<(u64, usize)> {
        while !self.data.len().is_multiple_of(16) {
            self.data.push(0);
        }
        let offset = self.data.len();
        self.data.resize(offset + size, 0);
        self.items.push(Item {
            type_index,
            offset: u32::try_from(offset)?,
            count,
            array,
        });
        Ok((u64::try_from(self.items.len())?, offset))
    }
    pub fn put(&mut self, at: usize, bytes: &[u8]) {
        self.data[at..at + bytes.len()].copy_from_slice(bytes);
    }
}
