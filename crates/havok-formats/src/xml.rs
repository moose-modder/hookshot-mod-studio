//! A tagfile's objects as the XML Havok's tools exchange (`hktagfile`, version 3), and the tools to compare two of them:
//! a parser and writer of the small XML subset that format uses ([`Element`]), the objects of a [`Tagfile`] as an
//! element tree ([`objects`]), the same tree made from a document written by other software ([`identity_form`]) and a
//! difference listing ([`diff`]). Together they make an independent reader's output comparable with this crate's
//! reading: write the file's objects here, have the other tool write its own XML of the same file, and the
//! differences are what the two read differently.
//!
//! # The form
//!
//! An `hktagfile` document has `type` elements and `object` elements. The objects are the part that is compared:
//!
//! - `<object id="object1" typeid="…">` holds one value of the object's class; the root is `object1` and the others are
//!   numbered in the order the first pointer to them is written, an object's members being written in turn, and
//!   `object0` is the null pointer;
//! - a class value is `<record>` with a `<field name="…">` for each serialised member (a class's own members after its
//!   parents', members the runtime keeps and the file does not store left out), each holding one value element;
//! - `<integer value>` (signed or unsigned as the type is), `<bool value="true|false">`, `<real hex="#…">` (the bits of
//!   the value as an `f64`, in lower case hexadecimal without leading zeros), `<string value>`, `<pointer id>`, and
//!   `<array count elementtypeid>` with one value element for each element. A fixed array of fixed arrays is written as
//!   one array of the innermost elements.
//!
//! [`objects`] writes `typeid` and `elementtypeid` as a type's *identity* (its name, then its template arguments in
//! angle brackets: `hkArray<hkReal,hkContainerHeapAllocator>`), which names the type the same way in any file;
//! other writers number their `type` elements (`type34`), so [`identity_form`] replaces the numbers by identities, drops
//! the `type` elements and the decimal `dec` rendering of a real, and the two trees are then equal exactly when the objects are.
//!
//! The XML tools of the games' ecosystem (HKLib) decode the bytes of a string as ISO 8859-1, so a string with a
//! non-ASCII character comes out of them as one character a byte: [`latin1_strings`] makes this crate's tree say the
//! same, for comparing against them.
//!
//! # What is not here
//!
//! The XML does not carry a file's type table in a form that rebuilds it (the `type` elements describe the other
//! tool's registry), so an XML document cannot be turned back into a tagfile; the round trip is text to tree to text
//! ([`Element::parse`], [`Element::write`]) and tagfile to tree, which a tagfile written back by [`crate::TagfileImage`]
//! can be checked against with [`diff_tagfiles`].

use std::collections::BTreeMap;

use crate::{HALF_FORMAT, Result, Tagfile, Value, invalid};

/// An element of an XML document: its name, attributes in order, and child elements. Text and comments are not kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Element {
    /// The element's name.
    pub name: String,
    /// The attributes, in the order written.
    pub attributes: Vec<(String, String)>,
    /// The child elements, in order.
    pub children: Vec<Element>,
}

/// How deeply the parser lets elements nest.
const MAX_DEPTH: usize = 128;

impl Element {
    /// An element with no attributes or children.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    /// The element with the attribute `key` added.
    #[must_use]
    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.attributes.push((key.to_owned(), value.into()));
        self
    }

    /// The value of the attribute `key`.
    pub fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The first child element named `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Parse a document: one root element. The XML declaration and comments are skipped, character data between
    /// elements must be white space (the format has none), and elements may nest 128 deep.
    pub fn parse(text: &str) -> Result<Element> {
        let mut parser = Parser { text, position: 0 };
        parser.document()
    }

    /// The element as indented XML text (two spaces a level, `<name … />` for an element with no children), without a
    /// declaration.
    pub fn write(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out, 0);
        out
    }

    fn write_into(&self, out: &mut String, depth: usize) {
        for _ in 0..depth {
            out.push_str("  ");
        }
        out.push('<');
        out.push_str(&self.name);
        for (key, value) in &self.attributes {
            out.push(' ');
            out.push_str(key);
            out.push_str("=\"");
            escape_into(out, value);
            out.push('"');
        }
        if self.children.is_empty() {
            out.push_str(" />\n");
            return;
        }
        out.push_str(">\n");
        for child in &self.children {
            child.write_into(out, depth.saturating_add(1));
        }
        for _ in 0..depth {
            out.push_str("  ");
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push_str(">\n");
    }
}

fn escape_into(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

struct Parser<'s> {
    text: &'s str,
    position: usize,
}

impl<'s> Parser<'s> {
    fn error(&self, detail: impl Into<String>) -> crate::Error {
        invalid(self.position, detail)
    }

    fn rest(&self) -> &'s str {
        self.text.get(self.position..).unwrap_or_default()
    }

    fn peek(&self) -> Option<u8> {
        self.rest().as_bytes().first().copied()
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn advance(&mut self, bytes: usize) {
        self.position = self.position.saturating_add(bytes);
    }

    fn skip_space(&mut self) {
        let rest = self.rest();
        let trimmed = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        self.advance(rest.len().saturating_sub(trimmed.len()));
    }

    /// Move past the next `end`.
    fn skip_past(&mut self, end: &str) -> Result<()> {
        match self.rest().find(end) {
            Some(at) => {
                self.advance(at.saturating_add(end.len()));
                Ok(())
            }
            None => Err(self.error(format!("no {end} to close the construct"))),
        }
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        if self.peek() == Some(byte) {
            self.advance(1);
            Ok(())
        } else {
            Err(self.error(format!("expected {:?}", char::from(byte))))
        }
    }

    fn name(&mut self) -> Result<String> {
        let rest = self.rest();
        let length = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')))
            .unwrap_or(rest.len());
        if length == 0 {
            return Err(self.error("expected a name"));
        }
        let name = rest.get(..length).unwrap_or_default().to_owned();
        self.advance(length);
        Ok(name)
    }

    /// A quoted attribute value, its entities resolved.
    fn quoted(&mut self) -> Result<String> {
        let quote = match self.peek() {
            Some(q @ (b'"' | b'\'')) => q,
            _ => return Err(self.error("expected a quoted attribute value")),
        };
        self.advance(1);
        let rest = self.rest();
        let Some(length) = rest.find(char::from(quote)) else {
            return Err(self.error("an attribute value is not closed"));
        };
        let raw = rest.get(..length).unwrap_or_default();
        let value = unescape(raw).map_err(|detail| self.error(detail))?;
        self.advance(length.saturating_add(1));
        Ok(value)
    }

    fn document(&mut self) -> Result<Element> {
        if self.starts_with("\u{feff}") {
            self.advance("\u{feff}".len());
        }
        let mut stack: Vec<Element> = Vec::new();
        let mut root: Option<Element> = None;
        loop {
            self.skip_space();
            let Some(byte) = self.peek() else {
                break;
            };
            if byte != b'<' {
                return Err(self.error("character data is not part of the format"));
            }
            if self.starts_with("<?") {
                self.skip_past("?>")?;
            } else if self.starts_with("<!--") {
                self.skip_past("-->")?;
            } else if self.starts_with("</") {
                self.advance(2);
                let name = self.name()?;
                self.skip_space();
                self.expect(b'>')?;
                let Some(done) = stack.pop() else {
                    return Err(self.error(format!("</{name}> closes nothing")));
                };
                if done.name != name {
                    return Err(self.error(format!("</{name}> closes <{}>", done.name)));
                }
                self.attach(&mut stack, &mut root, done)?;
            } else {
                self.advance(1);
                let mut element = Element::new(&self.name()?);
                let self_closing = loop {
                    self.skip_space();
                    match self.peek() {
                        Some(b'/') => {
                            self.advance(1);
                            self.expect(b'>')?;
                            break true;
                        }
                        Some(b'>') => {
                            self.advance(1);
                            break false;
                        }
                        Some(_) => {
                            let key = self.name()?;
                            self.skip_space();
                            self.expect(b'=')?;
                            self.skip_space();
                            let value = self.quoted()?;
                            element.attributes.push((key, value));
                        }
                        None => return Err(self.error("a tag is not closed")),
                    }
                };
                if self_closing {
                    self.attach(&mut stack, &mut root, element)?;
                } else if stack.len() >= MAX_DEPTH {
                    return Err(self.error(format!("elements nest more than {MAX_DEPTH} deep")));
                } else {
                    stack.push(element);
                }
            }
        }
        if let Some(open) = stack.last() {
            return Err(self.error(format!("<{}> is not closed", open.name)));
        }
        root.ok_or_else(|| self.error("no root element"))
    }

    /// Put a finished element under its parent, or make it the root.
    fn attach(&self, stack: &mut [Element], root: &mut Option<Element>, element: Element) -> Result<()> {
        match stack.last_mut() {
            Some(parent) => parent.children.push(element),
            None => {
                if root.replace(element).is_some() {
                    return Err(self.error("a second root element"));
                }
            }
        }
        Ok(())
    }
}

/// The text with its entities (`&amp; &lt; &gt; &quot; &apos; &#N; &#xN;`) replaced.
fn unescape(raw: &str) -> std::result::Result<String, String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let after = rest.get(at.saturating_add(1)..).unwrap_or_default();
        let Some(end) = after.find(';') else {
            return Err("an entity is not closed".to_owned());
        };
        let entity = after.get(..end).unwrap_or_default();
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            number => {
                let code = match number.strip_prefix("#x").or_else(|| number.strip_prefix("#X")) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()),
                };
                let c = code
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("unknown entity &{number};"))?;
                out.push(c);
            }
        }
        rest = after.get(end.saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    Ok(out)
}

// ---- a tagfile's objects

/// The name of the type `index` with its template arguments in angle brackets, the way a type is identified across
/// files: `hkArray<hkReal,hkContainerHeapAllocator>`.
fn identity(tag: &Tagfile<'_>, index: usize, depth: usize) -> Result<String> {
    let t = tag.types.get(index).filter(|_| depth < 32).ok_or_else(|| {
        invalid(
            0,
            format!("type {index} is out of bounds or nests more than 32 deep"),
        )
    })?;
    if t.parameters.is_empty() {
        return Ok(t.name.clone());
    }
    let mut arguments = Vec::with_capacity(t.parameters.len());
    for (name, value) in &t.parameters {
        arguments.push(if name.starts_with('t') {
            let target = usize::try_from(*value)
                .map_err(|_| invalid(0, "a type argument exceeds the address space"))?;
            identity(tag, target, depth.saturating_add(1))?
        } else {
            value.to_string()
        });
    }
    Ok(format!("{}<{}>", t.name, arguments.join(",")))
}

struct Walker<'t, 'a> {
    tag: &'t Tagfile<'a>,
    /// An object's DATA offset to its number.
    ids: BTreeMap<usize, usize>,
    /// The objects in number order.
    queue: Vec<Value<'t, 'a>>,
}

impl<'t, 'a> Walker<'t, 'a> {
    fn object_id(&mut self, target: Option<Value<'t, 'a>>) -> String {
        let Some(object) = target else {
            return "object0".to_owned();
        };
        if let Some(id) = self.ids.get(&object.offset) {
            return format!("object{id}");
        }
        let id = self.ids.len().saturating_add(1);
        self.ids.insert(object.offset, id);
        self.queue.push(object);
        format!("object{id}")
    }

    /// The members of a class value that the file stores: the parents' first.
    fn serialised_fields(&self, class: usize) -> Vec<&'t crate::Field> {
        let mut chain = Vec::new();
        let mut index = class;
        while index != 0 && chain.len() < 64 {
            let Some(t) = self.tag.types.get(index) else {
                break;
            };
            chain.push(t);
            index = t.parent;
        }
        chain
            .into_iter()
            .rev()
            .flat_map(|t| t.fields.iter())
            .filter(|f| f.flags & 1 == 0)
            .collect()
    }

    fn value(&mut self, value: Value<'t, 'a>, parent: &mut Element, depth: usize) -> Result<()> {
        if depth > 64 {
            return Err(invalid(value.at(), "values nest more than 64 deep"));
        }
        let format = value.format()?;
        let element = match format & 15 {
            2 => Element::new("bool").with("value", if value.unsigned()? == 0 { "false" } else { "true" }),
            4 => Element::new("integer").with(
                "value",
                if format & 512 == 0 {
                    value.unsigned()?.to_string()
                } else {
                    value.signed()?.to_string()
                },
            ),
            5 => {
                let real = match format {
                    0x17_4605 => f64::from(value.float()?),
                    HALF_FORMAT => f64::from(value.half()?),
                    other => {
                        return Err(invalid(
                            value.at(),
                            format!(
                                "a real of format {other:#x}: only 32-bit floats and Havok halves are stored"
                            ),
                        ));
                    }
                };
                Element::new("real").with("hex", format!("#{:x}", real.to_bits()))
            }
            3 => Element::new("string").with("value", value.string()?),
            6 => {
                let id = self.object_id(value.pointer()?);
                Element::new("pointer").with("id", id)
            }
            7 => {
                let mut record = Element::new("record");
                for field in self.serialised_fields(value.type_index) {
                    let mut member = Element::new("field").with("name", field.name.as_str());
                    self.value(value.field(&field.name)?, &mut member, depth.saturating_add(1))?;
                    record.children.push(member);
                }
                record
            }
            8 => self.array(value, depth)?,
            _ => {
                return Err(invalid(
                    value.at(),
                    format!("a {} of format {format:#x}", value.type_name()),
                ));
            }
        };
        parent.children.push(element);
        Ok(())
    }

    /// An array value: the element type of the declaration, and the elements, a fixed array of fixed arrays flattened.
    fn array(&mut self, value: Value<'t, 'a>, depth: usize) -> Result<Element> {
        let mut element_type = self.tag.inherited(value.type_index, |t| t.subtype)?;
        let mut leaves = Vec::new();
        flatten(value, &mut leaves, 0)?;
        if element_type == 0 {
            return Err(invalid(value.at(), "an array with no element type"));
        }
        // a fixed array of fixed arrays is written as one array of the innermost elements
        let mut nested = 0usize;
        while nested < 8
            && self.tag.format(element_type)? & 15 == 8
            && self.tag.format(element_type)? & 32 != 0
            && self.tag.format(value.type_index)? & 32 != 0
        {
            element_type = self.tag.inherited(element_type, |t| t.subtype)?;
            nested = nested.saturating_add(1);
        }
        let mut array = Element::new("array")
            .with("count", leaves.len().to_string())
            .with("elementtypeid", identity(self.tag, element_type, 0)?);
        for leaf in leaves {
            self.value(leaf, &mut array, depth.saturating_add(1))?;
        }
        Ok(array)
    }
}

/// The elements of an array value; for a fixed array whose elements are fixed arrays, their elements in turn.
fn flatten<'t, 'a>(value: Value<'t, 'a>, out: &mut Vec<Value<'t, 'a>>, depth: usize) -> Result<()> {
    if depth > 8 {
        return Err(invalid(value.at(), "fixed arrays nest more than 8 deep"));
    }
    let fixed = value.format()? & 32 != 0;
    let array = value.array()?;
    for i in 0..array.len() {
        let element = array.get(i)?;
        let inner = element.format()?;
        if fixed && inner & 15 == 8 && inner & 32 != 0 {
            flatten(element, out, depth.saturating_add(1))?;
        } else {
            out.push(element);
        }
    }
    Ok(())
}

/// The objects of `tag` as an `hktagfile` element: an `object` for the root and each object reachable from it, numbered
/// in the order of first reference (see the [module documentation](self)). Types are named by identity; the
/// document has no `type` elements.
pub fn objects(tag: &Tagfile<'_>) -> Result<Element> {
    let mut walker = Walker {
        tag,
        ids: BTreeMap::new(),
        queue: Vec::new(),
    };
    walker.object_id(Some(tag.root()?));
    let mut document = Element::new("hktagfile").with("version", "3");
    let mut next = 0usize;
    while let Some(&value) = walker.queue.get(next) {
        next = next.saturating_add(1);
        let mut object = Element::new("object")
            .with("id", format!("object{next}"))
            .with("typeid", identity(tag, value.type_index, 0)?);
        walker.value(value, &mut object, 0)?;
        document.children.push(object);
    }
    Ok(document)
}

// ---- other writers' documents

fn type_identity(
    id: &str,
    types: &BTreeMap<&str, &Element>,
    memo: &mut BTreeMap<String, String>,
    depth: usize,
) -> Result<String> {
    if let Some(known) = memo.get(id) {
        return Ok(known.clone());
    }
    if depth >= 32 {
        return Err(invalid(0, format!("type {id} nests more than 32 deep")));
    }
    let element = types
        .get(id)
        .ok_or_else(|| invalid(0, format!("the document names no type {id}")))?;
    let name = element
        .child("name")
        .and_then(|n| n.attribute("value"))
        .ok_or_else(|| invalid(0, format!("type {id} has no name")))?;
    let mut arguments = Vec::new();
    if let Some(parameters) = element.child("parameters") {
        for parameter in &parameters.children {
            match parameter.name.as_str() {
                "typeparam" => {
                    let target = parameter
                        .attribute("id")
                        .ok_or_else(|| invalid(0, format!("a type parameter of {id} has no id")))?;
                    arguments.push(type_identity(target, types, memo, depth.saturating_add(1))?);
                }
                "valueparam" => arguments.push(
                    parameter
                        .attribute("value")
                        .ok_or_else(|| invalid(0, format!("a value parameter of {id} has no value")))?
                        .to_owned(),
                ),
                other => return Err(invalid(0, format!("a {other} among the parameters of type {id}"))),
            }
        }
    }
    let identity = if arguments.is_empty() {
        name.to_owned()
    } else {
        format!("{name}<{}>", arguments.join(","))
    };
    memo.insert(id.to_owned(), identity.clone());
    Ok(identity)
}

fn identify(
    element: &Element,
    types: &BTreeMap<&str, &Element>,
    memo: &mut BTreeMap<String, String>,
) -> Result<Element> {
    let mut out = Element::new(&element.name);
    for (key, value) in &element.attributes {
        match (element.name.as_str(), key.as_str()) {
            ("real", "dec") => {}
            (_, "typeid" | "elementtypeid") => {
                out.attributes
                    .push((key.clone(), type_identity(value, types, memo, 0)?));
            }
            _ => out.attributes.push((key.clone(), value.clone())),
        }
    }
    for child in &element.children {
        out.children.push(identify(child, types, memo)?);
    }
    Ok(out)
}

/// The objects of an `hktagfile` document written by other software, in the form [`objects`] makes: the `object` elements
/// with every `typeid` and `elementtypeid` replaced by the type's identity and the decimal `dec` of a real dropped;
/// the `type` elements are dropped.
pub fn identity_form(document: &Element) -> Result<Element> {
    if document.name != "hktagfile" {
        return Err(invalid(
            0,
            format!("expected an hktagfile, found a {}", document.name),
        ));
    }
    let types: BTreeMap<&str, &Element> = document
        .children
        .iter()
        .filter(|c| c.name == "type")
        .filter_map(|c| c.attribute("id").map(|id| (id, c)))
        .collect();
    let mut memo = BTreeMap::new();
    let mut out = Element::new("hktagfile").with("version", document.attribute("version").unwrap_or("3"));
    for object in document.children.iter().filter(|c| c.name == "object") {
        out.children.push(identify(object, &types, &mut memo)?);
    }
    Ok(out)
}

/// The tree with every string value as the tools that decode a string's bytes as ISO 8859-1 write it: one character
/// for each byte of the UTF-8 text.
pub fn latin1_strings(element: &Element) -> Element {
    let mut out = Element::new(&element.name);
    for (key, value) in &element.attributes {
        if element.name == "string" && key == "value" {
            out.attributes
                .push((key.clone(), value.bytes().map(char::from).collect()));
        } else {
            out.attributes.push((key.clone(), value.clone()));
        }
    }
    out.children = element.children.iter().map(latin1_strings).collect();
    out
}

// ---- differences

/// Where two trees differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    /// The path to the element: the object, then each element's name with a `field` element's name in parentheses and
    /// its position among the siblings.
    pub path: String,
    /// What the left tree has there.
    pub left: String,
    /// What the right tree has there.
    pub right: String,
}

fn attributes_text(element: &Element) -> String {
    element
        .attributes
        .iter()
        .map(|(k, v)| format!("{k}={v:?}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first `limit` differences between two trees compared element by element (name, attributes, number of children,
/// then the children in order); where two elements have different numbers of children the children are not compared.
pub fn diff(left: &Element, right: &Element, limit: usize) -> Vec<Difference> {
    let mut out = Vec::new();
    let mut stack = vec![(left, right, left.name.clone())];
    while let Some((l, r, path)) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        if l.name != r.name || l.attributes != r.attributes {
            out.push(Difference {
                path,
                left: format!("{} {}", l.name, attributes_text(l)),
                right: format!("{} {}", r.name, attributes_text(r)),
            });
            continue;
        }
        if l.children.len() != r.children.len() {
            out.push(Difference {
                path,
                left: format!("{} children", l.children.len()),
                right: format!("{} children", r.children.len()),
            });
            continue;
        }
        // the first difference in document order is found first
        for (index, (a, b)) in l.children.iter().zip(&r.children).enumerate().rev() {
            let label = match a.attribute("name").or_else(|| a.attribute("id")) {
                Some(name) => format!("{}({name})[{index}]", a.name),
                None => format!("{}[{index}]", a.name),
            };
            stack.push((a, b, format!("{path}/{label}")));
        }
    }
    out
}

/// The first `limit` differences between the objects of two tagfiles (see [`diff`]): what a writer changed in a file
/// it rewrote, or what two readings of one file disagree about.
pub fn diff_tagfiles(left: &Tagfile<'_>, right: &Tagfile<'_>, limit: usize) -> Result<Vec<Difference>> {
    Ok(diff(&objects(left)?, &objects(right)?, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_parses_to_a_tree_and_writes_back() -> Result<()> {
        let text = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<a x=\"1\">\n  <!-- a comment -->\n  <b name=\"p&amp;q &lt;&#65;&#x42;&quot;\" />\n  <c></c>\n</a>\n";
        let tree = Element::parse(text)?;
        assert_eq!(tree.name, "a");
        assert_eq!(tree.attribute("x"), Some("1"));
        assert_eq!(tree.children.len(), 2);
        assert_eq!(tree.children[0].attribute("name"), Some("p&q <AB\""));
        assert!(tree.children[1].children.is_empty());
        let written = tree.write();
        assert_eq!(
            written,
            "<a x=\"1\">\n  <b name=\"p&amp;q &lt;AB&quot;\" />\n  <c />\n</a>\n"
        );
        assert_eq!(Element::parse(&written)?, tree);
        Ok(())
    }

    #[test]
    fn a_document_that_is_not_the_format_is_refused() {
        for (text, why) in [
            ("", "no root element"),
            ("<a>", "<a> is not closed"),
            ("<a></b>", "</b> closes <a>"),
            ("</a>", "</a> closes nothing"),
            ("<a/><b/>", "a second root element"),
            ("<a>text</a>", "character data is not part of the format"),
            ("<a x=1/>", "expected a quoted attribute value"),
            ("<a x=\"&nope;\"/>", "unknown entity &nope;"),
            ("<a x=\"&amp\"/>", "an entity is not closed"),
            ("<a x=\"1/>", "an attribute value is not closed"),
            ("<a", "a tag is not closed"),
        ] {
            let error = Element::parse(text).expect_err(text);
            assert!(error.detail.contains(why), "{text:?}: {error}");
        }
    }

    #[test]
    fn elements_nest_up_to_a_limit() {
        let deep = |n: usize| format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
        assert!(Element::parse(&deep(MAX_DEPTH)).is_ok());
        let error = Element::parse(&deep(MAX_DEPTH + 1)).expect_err("too deep");
        assert!(error.detail.contains("nest more than"), "{error}");
    }

    #[test]
    fn identity_form_names_types_and_drops_what_other_writers_add() -> Result<()> {
        let document = Element::parse(
            r##"<hktagfile version="3">
              <type id="type1"><name value="hkArray" /><parameters count="2"><typeparam id="type2" /><typeparam id="type3" /></parameters></type>
              <type id="type2"><name value="hkReal" /></type>
              <type id="type3"><name value="hkContainerHeapAllocator" /></type>
              <type id="type4"><name value="T[N]" /><parameters count="2"><typeparam id="type2" /><valueparam value="4" /></parameters></type>
              <object id="object1" typeid="type1">
                <array count="1" elementtypeid="type2"><real dec="0.5" hex="#3fe0000000000000" /></array>
              </object>
              <object id="object2" typeid="type4" />
            </hktagfile>"##,
        )?;
        let form = identity_form(&document)?;
        assert_eq!(form.children.len(), 2);
        assert_eq!(
            form.children[0].attribute("typeid"),
            Some("hkArray<hkReal,hkContainerHeapAllocator>")
        );
        assert_eq!(form.children[1].attribute("typeid"), Some("T[N]<hkReal,4>"));
        let array = &form.children[0].children[0];
        assert_eq!(array.attribute("elementtypeid"), Some("hkReal"));
        assert_eq!(
            array.children[0].attributes,
            [("hex".to_owned(), "#3fe0000000000000".to_owned())]
        );
        let missing = Element::parse(r#"<hktagfile><object id="object1" typeid="type9" /></hktagfile>"#)?;
        assert!(identity_form(&missing).is_err());
        assert!(identity_form(&Element::new("other")).is_err());
        Ok(())
    }

    #[test]
    fn strings_are_shown_as_one_character_a_byte() {
        let tree = Element::new("record").with("value", "é").with("name", "é");
        let string = Element::new("string").with("value", "é");
        let marked = latin1_strings(&Element {
            children: vec![string, tree],
            ..Element::new("root")
        });
        assert_eq!(marked.children[0].attribute("value"), Some("\u{c3}\u{a9}"));
        // only the value of a string element changes
        assert_eq!(marked.children[1].attribute("value"), Some("é"));
        assert_eq!(marked.children[1].attribute("name"), Some("é"));
    }

    #[test]
    fn differences_are_listed_in_document_order_with_their_paths() -> Result<()> {
        let a = Element::parse(
            r#"<hktagfile><object id="object1"><record><field name="x"><integer value="1" /></field><field name="y"><integer value="2" /></field></record></object></hktagfile>"#,
        )?;
        assert!(diff(&a, &a, 10).is_empty());
        let b = Element::parse(
            r#"<hktagfile><object id="object1"><record><field name="x"><integer value="1" /></field><field name="y"><integer value="3" /></field></record></object></hktagfile>"#,
        )?;
        let found = diff(&a, &b, 10);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].path,
            "hktagfile/object(object1)[0]/record[0]/field(y)[1]/integer[0]"
        );
        assert_eq!(found[0].left, "integer value=\"2\"");
        assert_eq!(found[0].right, "integer value=\"3\"");
        let c = Element::parse(r#"<hktagfile><object id="object1"><record /></object></hktagfile>"#)?;
        let found = diff(&a, &c, 10);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].left, "2 children");
        assert_eq!(diff(&a, &b, 0).len(), 0);
        Ok(())
    }
}
