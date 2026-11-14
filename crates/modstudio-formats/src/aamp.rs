//! AAMP version 2 parameter archives (little-endian): what a cloth file carries after its tagfile (the simulated
//! meshes and colliders, by name; `Phive::params`). Read into lists, objects and parameters, and written back in the
//! layout the game's files have, so an archive read and written again comes back byte for byte (the 171 cloth
//! files' do).
//!
//! Layout: a 0x30-byte header (`AAMP`, then eleven words: version, flags, size, the parameter IO version, the length
//! of the type name after the header, the counts of lists, objects and parameters, the sizes of the data and string
//! sections, and one more word); the type name; every list (12 bytes: hash, offset to its child lists, their count,
//! offset to its objects, their count; offsets in words from the list), breadth first; every object (8 bytes: hash,
//! offset to its parameters, their count); every parameter (8 bytes: hash, then the value's type in the top byte and
//! its offset in words from the parameter); the data section (each fixed-size value once, in parameter order); the
//! string section (each string once, with its terminator, padded to 4 bytes).

use crate::bytes::{ReadLe, check_magic};
use crate::error::{Result, invalid, unsupported};

const HEADER: usize = 0x30;

/// A list: child lists and objects.
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub hash: u32,
    pub lists: Vec<List>,
    pub objects: Vec<Object>,
}

/// An object: parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub hash: u32,
    pub params: Vec<Param>,
}

/// A parameter: its type and value as stored (a string without its terminator).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub hash: u32,
    pub kind: u8,
    pub value: Vec<u8>,
}

impl Param {
    /// The value as text, for a string parameter.
    pub fn text(&self) -> Option<&str> {
        value_size(self.kind)
            .is_none()
            .then(|| std::str::from_utf8(&self.value).ok())
            .flatten()
    }
}

/// An archive.
#[derive(Debug, Clone, PartialEq)]
pub struct Aamp {
    pub flags: u32,
    pub pio_version: u32,
    /// The header's last word, as stored.
    pub reserved: u32,
    /// The type name after the header, as stored (its terminator and padding included).
    pub type_name: Vec<u8>,
    pub root: List,
}

/// The size of a value of type `kind`, or `None` for a string (stored up to its terminator).
fn value_size(kind: u8) -> Option<usize> {
    match kind {
        0 | 1 | 2 | 11 | 21 | 22 => Some(4),
        3 | 12 => Some(8),
        4 | 13 => Some(12),
        5 | 6 | 14 | 15 | 17 => Some(16),
        16 => Some(32),
        18 => Some(64),
        19 => Some(128),
        _ => None,
    }
}

/// The hash of a name, as archives key lists, objects and parameters: its CRC-32.
pub fn hash(name: &str) -> u32 {
    crate::restbl::crc32(name.as_bytes())
}

impl Aamp {
    /// Reads an archive (bytes past the size its header gives are left: a cloth file pads it).
    pub fn read(data: &[u8]) -> Result<Aamp> {
        check_magic(data, 0, "AAMP")?;
        let word = |i: usize| data.u32_at(4 + 4 * i);
        if word(0)? != 2 {
            return Err(unsupported(format!("AAMP version {}", word(0)?)));
        }
        let size = word(2)? as usize;
        if size > data.len() {
            return Err(invalid(format!("AAMP of {size} bytes in {}", data.len())));
        }
        let data = &data[..size];
        let name_len = word(4)? as usize;
        let mut reader = Reader { data, depth: 0 };
        Ok(Aamp {
            flags: word(1)?,
            pio_version: word(3)?,
            reserved: word(10)?,
            type_name: data.slice_at(HEADER, name_len)?.to_vec(),
            root: reader.list(HEADER + name_len)?,
        })
    }

    /// The archive in the game's layout (see the module's notes).
    pub fn write(&self) -> Vec<u8> {
        // lists breadth first: each list's children follow one another
        let mut lists: Vec<&List> = vec![&self.root];
        let mut i = 0;
        while i < lists.len() {
            let children = &lists[i].lists;
            lists.extend(children.iter());
            i += 1;
        }
        let objects: Vec<&Object> = lists.iter().flat_map(|l| l.objects.iter()).collect();
        let params: Vec<&Param> = objects.iter().flat_map(|o| o.params.iter()).collect();
        let list_base = HEADER + self.type_name.len();
        let object_base = list_base + 12 * lists.len();
        let param_base = object_base + 8 * objects.len();
        let data_base = param_base + 8 * params.len();
        // the data section: each fixed-size value once; the string section: each string once
        let mut data = Vec::new();
        let mut data_at: Vec<(Vec<u8>, usize)> = Vec::new();
        let mut strings = Vec::new();
        let mut string_at: Vec<(Vec<u8>, usize)> = Vec::new();
        let mut value_at = Vec::with_capacity(params.len());
        for p in &params {
            if value_size(p.kind).is_some() {
                let at = match data_at.iter().find(|(v, _)| *v == p.value) {
                    Some((_, at)) => *at,
                    None => {
                        data_at.push((p.value.clone(), data.len()));
                        data.extend_from_slice(&p.value);
                        data.len() - p.value.len()
                    }
                };
                value_at.push((false, at));
            } else {
                value_at.push((true, 0));
            }
        }
        for p in params.iter().filter(|p| value_size(p.kind).is_none()) {
            if !string_at.iter().any(|(v, _)| *v == p.value) {
                string_at.push((p.value.clone(), strings.len()));
                strings.extend_from_slice(&p.value);
                strings.push(0);
                while strings.len() % 4 != 0 {
                    strings.push(0);
                }
            }
        }
        let string_base = data_base + data.len();
        let mut out = vec![0u8; string_base + strings.len()];
        out[..4].copy_from_slice(b"AAMP");
        let header = [
            2,
            self.flags,
            out.len() as u32,
            self.pio_version,
            self.type_name.len() as u32,
            lists.len() as u32,
            objects.len() as u32,
            params.len() as u32,
            data.len() as u32,
            strings.len() as u32,
            self.reserved,
        ];
        for (k, w) in header.iter().enumerate() {
            put32(&mut out, 4 + 4 * k, *w);
        }
        out[HEADER..HEADER + self.type_name.len()].copy_from_slice(&self.type_name);
        // a list's children start where the next list not yet placed is (its objects likewise), even when it has none
        let (mut next_list, mut next_object) = (1, 0);
        for (k, l) in lists.iter().enumerate() {
            let at = list_base + 12 * k;
            let lists_at = (list_base + 12 * next_list - at) / 4;
            let objects_at = (object_base + 8 * next_object - at) / 4;
            next_list += l.lists.len();
            next_object += l.objects.len();
            put32(&mut out, at, l.hash);
            put16(&mut out, at + 4, lists_at as u16);
            put16(&mut out, at + 6, l.lists.len() as u16);
            put16(&mut out, at + 8, objects_at as u16);
            put16(&mut out, at + 10, l.objects.len() as u16);
        }
        let mut next_param = 0;
        for (k, o) in objects.iter().enumerate() {
            let at = object_base + 8 * k;
            put32(&mut out, at, o.hash);
            put16(
                &mut out,
                at + 4,
                ((param_base + 8 * next_param - at) / 4) as u16,
            );
            put16(&mut out, at + 6, o.params.len() as u16);
            next_param += o.params.len();
        }
        for (k, p) in params.iter().enumerate() {
            let at = param_base + 8 * k;
            let value = match value_at[k] {
                (false, rel) => data_base + rel,
                (true, _) => {
                    string_base
                        + string_at
                            .iter()
                            .find(|(v, _)| *v == p.value)
                            .map_or(0, |(_, a)| *a)
                }
            };
            put32(&mut out, at, p.hash);
            put32(
                &mut out,
                at + 4,
                (u32::from(p.kind) << 24) | ((value - at) / 4) as u32,
            );
        }
        out[data_base..data_base + data.len()].copy_from_slice(&data);
        out[string_base..].copy_from_slice(&strings);
        out
    }
}

fn put32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put16(out: &mut [u8], at: usize, v: u16) {
    out[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

struct Reader<'a> {
    data: &'a [u8],
    depth: usize,
}

impl Reader<'_> {
    fn list(&mut self, at: usize) -> Result<List> {
        self.depth += 1;
        if self.depth > 64 {
            return Err(invalid("AAMP lists nested deeper than 64"));
        }
        let hash = self.data.u32_at(at)?;
        let lists_at = at + 4 * self.data.u16_at(at + 4)? as usize;
        let list_count = self.data.u16_at(at + 6)? as usize;
        let objects_at = at + 4 * self.data.u16_at(at + 8)? as usize;
        let object_count = self.data.u16_at(at + 10)? as usize;
        let mut lists = Vec::with_capacity(list_count);
        for i in 0..list_count {
            lists.push(self.list(lists_at + 12 * i)?);
        }
        let mut objects = Vec::with_capacity(object_count);
        for i in 0..object_count {
            objects.push(self.object(objects_at + 8 * i)?);
        }
        self.depth -= 1;
        Ok(List {
            hash,
            lists,
            objects,
        })
    }

    fn object(&self, at: usize) -> Result<Object> {
        let hash = self.data.u32_at(at)?;
        let params_at = at + 4 * self.data.u16_at(at + 4)? as usize;
        let count = self.data.u16_at(at + 6)? as usize;
        let mut params = Vec::with_capacity(count);
        for i in 0..count {
            let p = params_at + 8 * i;
            let word = self.data.u32_at(p + 4)?;
            let kind = (word >> 24) as u8;
            let v = p + 4 * (word & 0x00FF_FFFF) as usize;
            let value = match value_size(kind) {
                Some(n) => self.data.slice_at(v, n)?.to_vec(),
                None => self.data.cstr_at(v)?.as_bytes().to_vec(),
            };
            params.push(Param {
                hash: self.data.u32_at(p)?,
                kind,
                value,
            });
        }
        Ok(Object { hash, params })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Aamp {
        let text = |name: &str, value: &str| Param {
            hash: hash(name),
            kind: 20,
            value: value.as_bytes().to_vec(),
        };
        let flag = |name: &str, on: bool| Param {
            hash: hash(name),
            kind: 0,
            value: u32::from(on).to_le_bytes().to_vec(),
        };
        Aamp {
            flags: 3,
            pio_version: 0,
            reserved: 0,
            type_name: b"xml\0".to_vec(),
            root: List {
                hash: hash("param_root"),
                lists: vec![
                    List {
                        hash: 1,
                        lists: vec![],
                        objects: vec![Object {
                            hash: 7,
                            params: vec![text("Name", "Mesh_A"), flag("Twist", false)],
                        }],
                    },
                    List {
                        hash: 2,
                        lists: vec![],
                        objects: (0..3)
                            .map(|k| Object {
                                hash: hash(&format!("collidable_{k}")),
                                params: vec![
                                    text("Name", &format!("Collidable_{k}")),
                                    flag("ForReplace", false),
                                ],
                            })
                            .collect(),
                    },
                ],
                objects: vec![],
            },
        }
    }

    #[test]
    fn an_archive_written_reads_back_the_same_and_writes_the_same_bytes() {
        let a = sample();
        let bytes = a.write();
        let b = Aamp::read(&bytes).unwrap();
        assert_eq!(b, a);
        assert_eq!(b.write(), bytes);
        // a value used twice is stored once: four `false`s, one word
        assert_eq!(bytes.u32_at(4 + 4 * 8).unwrap(), 4);
        // padding after the archive (as a cloth file has) is left
        let mut padded = bytes.clone();
        padded.extend([0; 12]);
        assert_eq!(Aamp::read(&padded).unwrap(), a);
    }

    #[test]
    fn strings_are_read_up_to_their_terminator() {
        let a = sample();
        let b = Aamp::read(&a.write()).unwrap();
        let p = &b.root.lists[1].objects[2].params[0];
        assert_eq!(p.text(), Some("Collidable_2"));
        assert_eq!(b.root.lists[1].objects[2].params[1].text(), None);
    }

    #[test]
    fn other_versions_are_refused() {
        let mut bytes = sample().write();
        bytes[4] = 1;
        assert!(Aamp::read(&bytes).is_err());
        assert!(Aamp::read(b"AAMP").is_err());
    }
}
