//! Behaviour tagfiles: typed views over the reflection-driven [`Value`] for the classes of Havok Behavior (`hkb*`) and
//! FromSoftware's `Custom*` classes that Sekiro's and Elden Ring's `behbnd` binders hold, and a reader of the three kinds
//! of file those binders contain.
//!
//! # The files
//!
//! Every behaviour tagfile of either install is a `TAG0` tagfile whose root container (`hkRootLevelContainer`) has exactly
//! one named variant, and the variant's class says what the file is ([`FileKind`]): a behaviour graph (`hkbBehaviorGraph`,
//! its data, string data and initial variable values, and the nodes under its root generator), a character
//! (`hkbCharacterData`, its string data and the character properties' values) or a project (`hkbProjectData` and its string
//! data). All of them are in `chr/*.behbnd.dcx` (members `Behaviors/*.hkx`, `Characters/*.hkx` and the project file); no other
//! binder of either install holds an `hkb*` class. Their type tables are the file's own (no compendium). Sekiro's
//! `c9996.hkx` behaviour is a packfile and is not read here.
//!
//! # The views
//!
//! A view ([`View`]) is a [`Value`] of a class with one accessor for each serialised member of the class (its own and the
//! members it inherits), named in snake case after the member, returning a typed result: integers, floats, vectors,
//! strings, an [`EnumValue`] for enumerations and flag sets (the member names come from [`enums`]), an [`Object`] for a
//! pointer to a class that other classes derive from, and a view of the target for a pointer to a class nothing derives
//! from. [`Typed::of`] makes the view of a value from its class name. The members that exist only in the runtime object
//! (flag bit 0 of a member in the type table) are not stored: they are zero in every object of both installs and have
//! no accessor, and neither has `hkReferencedObject::propertyBag`, which is 8 zero bytes in each of them.
//! A member that only one game's layout of a class has returns an `Option` (`None` where the layout lacks it).
//!
//! The views are generated from the type tables of the behaviour tagfiles of both installs (one layout for each class
//! name in each game, 93 classes in all) and the enum member names of HKLib's hk2018 classes; the generator is
//! `tools/gen_behavior.py`.
//!
//! # Reading a file
//!
//! [`Document::read`] classifies a tagfile and checks its root; [`Document::behavior`] reads a behaviour graph's variables,
//! events and variable bindings by name ([`Behavior`]) and [`Behavior::check`] verifies the counts and indices that tie
//! them together (the same checks hold on every behaviour graph of both installs, see the tests). [`class_counts`]
//! counts a file's objects and structure elements by class.
//!
//! ```no_run
//! use havok_formats::{Tagfile, behavior::{Document, FileKind}};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let bytes = std::fs::read("c0000.hkx")?;
//!     let tagfile = Tagfile::read(&bytes, None)?;
//!     let document = Document::read(&tagfile)?;
//!     if document.kind() == FileKind::Behavior {
//!         let behavior = document.behavior()?;
//!         for variable in behavior.variables()? {
//!             println!("{} {:?}", variable.name, variable.kind.name());
//!         }
//!     }
//!     Ok(())
//! }
//! ```

mod classes;
pub mod enums;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub use classes::*;
pub use enums::enum_info;

use crate::{ItemKind, Result, Tagfile, Value, invalid};

/// A class of the behaviour tagfiles as a typed view: a [`Value`] of that class or of a class derived from it.
pub trait View<'t, 'a>: Copy {
    /// The class name as a tagfile spells it (`Outer::Inner` for a nested structure).
    const CLASS: &'static str;
    /// The view of `value`, without checking its class.
    fn from_value(value: Value<'t, 'a>) -> Self;
    /// The value the view reads.
    fn raw(self) -> Value<'t, 'a>;
    /// The view of `value` when its class is, or derives from, [`CLASS`](View::CLASS).
    fn cast(value: Value<'t, 'a>) -> Option<Self> {
        is_a(value, Self::CLASS).then(|| Self::from_value(value))
    }
}

/// Whether `value`'s class is `class` or derives from it.
fn is_a(value: Value<'_, '_>, class: &str) -> bool {
    let types = &value.tag.types;
    let mut index = value.type_index;
    let mut steps = 0usize;
    while index != 0 && steps < 256 {
        let Some(t) = types.get(index) else {
            return false;
        };
        if t.name == class {
            return true;
        }
        index = t.parent;
        steps = steps.saturating_add(1);
    }
    false
}

/// The members of an enumeration or flag set a behaviour class uses, with the names HKLib's hk2018 classes give them.
#[derive(Debug)]
pub struct EnumInfo {
    /// The type name as a tagfile spells it, `Outer::Inner`.
    pub name: &'static str,
    /// Whether the type is a flag set (`hkFlags`) rather than an enumeration (`hkEnum`).
    pub flags: bool,
    /// The members: name and value.
    pub values: &'static [(&'static str, i64)],
}

impl EnumInfo {
    /// The name of the member whose value is `value` (the first, when several members share it).
    pub fn name_of(&self, value: i64) -> Option<&'static str> {
        self.values.iter().find(|&&(_, v)| v == value).map(|&(n, _)| n)
    }

    /// For a flag set: the names of the members with a non-zero value all of whose bits are set in `value`, and the bits
    /// of `value` that none of those members names.
    pub fn flag_names(&self, value: i64) -> (Vec<&'static str>, i64) {
        let mut names = Vec::new();
        let mut named = 0i64;
        for &(name, v) in self.values {
            if v != 0 && value & v == v {
                names.push(name);
                named |= v;
            }
        }
        (names, value & !named)
    }
}

/// The value of an enumeration or flag-set member as stored, with the table that names its values.
#[derive(Clone, Copy, Debug)]
pub struct EnumValue {
    /// The stored value, sign-extended from the member's width when it is signed.
    pub raw: i64,
    /// The table of the member's type; `None` for a type the tables lack.
    pub info: Option<&'static EnumInfo>,
}

impl EnumValue {
    /// The name of the member with exactly this value.
    pub fn name(self) -> Option<&'static str> {
        self.info?.name_of(self.raw)
    }

    /// Whether the value is one the table names: a member of the enumeration, or a flag set with no bit unnamed.
    pub fn is_named(self) -> bool {
        match self.info {
            Some(info) if info.flags => info.flag_names(self.raw).1 == 0,
            Some(info) => info.name_of(self.raw).is_some(),
            None => false,
        }
    }

    /// The names of the flags set (see [`EnumInfo::flag_names`]); empty when the table is missing.
    pub fn flag_names(self) -> Vec<&'static str> {
        self.info
            .map_or_else(Vec::new, |info| info.flag_names(self.raw).0)
    }
}

// ---- reading members: the helpers the generated accessors call

fn narrow<T>(value: Value<'_, '_>, n: impl std::fmt::Display, ok: Option<T>) -> Result<T> {
    ok.ok_or_else(|| {
        invalid(
            value.at(),
            format!(
                "{n} does not fit the {} the member is declared as",
                value.type_name()
            ),
        )
    })
}

fn int<T: TryFrom<i64>>(value: Value<'_, '_>) -> Result<T> {
    let n = value.signed()?;
    narrow(value, n, T::try_from(n).ok())
}

fn uint<T: TryFrom<u64>>(value: Value<'_, '_>) -> Result<T> {
    let n = value.unsigned()?;
    narrow(value, n, T::try_from(n).ok())
}

fn boolean(value: Value<'_, '_>) -> Result<bool> {
    Ok(value.unsigned()? != 0)
}

fn real(value: Value<'_, '_>) -> Result<f32> {
    value.float()
}

fn enumeration(value: Value<'_, '_>, type_name: &str) -> Result<EnumValue> {
    Ok(EnumValue {
        raw: value.signed()?,
        info: enum_info(type_name),
    })
}

fn object<'t, 'a>(value: Value<'t, 'a>) -> Result<Option<Object<'t, 'a>>> {
    Ok(value.pointer()?.map(Object))
}

fn pointer_view<'t, 'a, V: View<'t, 'a>>(value: Value<'t, 'a>) -> Result<Option<V>> {
    value.pointer()?.map(cast_pointee::<V>).transpose()
}

fn cast_pointee<'t, 'a, V: View<'t, 'a>>(target: Value<'t, 'a>) -> Result<V> {
    V::cast(target).ok_or_else(|| {
        invalid(
            target.at(),
            format!("expected a {} object, found a {}", V::CLASS, target.type_name()),
        )
    })
}

fn qs_transform(value: Value<'_, '_>) -> Result<[[f32; 4]; 3]> {
    Ok([
        value.field("translation")?.vector::<4>()?,
        value.field("rotation")?.vector::<4>()?,
        value.field("scale")?.vector::<4>()?,
    ])
}

/// Map every element of an array value.
fn each<'t, 'a, T>(value: Value<'t, 'a>, mut read: impl FnMut(Value<'t, 'a>) -> Result<T>) -> Result<Vec<T>> {
    let array = value.array()?;
    let mut out = Vec::with_capacity(array.len());
    for i in 0..array.len() {
        out.push(read(array.get(i)?)?);
    }
    Ok(out)
}

fn reals(value: Value<'_, '_>) -> Result<Vec<f32>> {
    each(value, |v| v.float())
}

fn real_rows(value: Value<'_, '_>) -> Result<Vec<Vec<f32>>> {
    each(value, reals)
}

fn ints<T: TryFrom<i64>>(value: Value<'_, '_>) -> Result<Vec<T>> {
    each(value, int::<T>)
}

fn uints<T: TryFrom<u64>>(value: Value<'_, '_>) -> Result<Vec<T>> {
    each(value, uint::<T>)
}

fn strings(value: Value<'_, '_>) -> Result<Vec<String>> {
    each(value, |v| v.string())
}

fn vectors(value: Value<'_, '_>) -> Result<Vec<[f32; 4]>> {
    each(value, |v| v.vector::<4>())
}

fn qs_transforms(value: Value<'_, '_>) -> Result<Vec<[[f32; 4]; 3]>> {
    each(value, qs_transform)
}

fn objects<'t, 'a>(value: Value<'t, 'a>) -> Result<Vec<Option<Object<'t, 'a>>>> {
    each(value, object)
}

fn pointer_views<'t, 'a, V: View<'t, 'a>>(value: Value<'t, 'a>) -> Result<Vec<Option<V>>> {
    each(value, pointer_view::<V>)
}

fn elements<'t, 'a, V: View<'t, 'a>>(value: Value<'t, 'a>) -> Result<Vec<V>> {
    each(value, |v| Ok(V::from_value(v)))
}

// ---- objects and their references

/// An object of a behaviour tagfile: a [`Value`] known to be a class instance (what a pointer refers to), read by its
/// class name.
#[derive(Clone, Copy, Debug)]
pub struct Object<'t, 'a>(Value<'t, 'a>);

/// A pointer member of an object and the object it refers to.
#[derive(Clone, Debug)]
pub struct Reference<'t, 'a> {
    /// The member's path from the object: `name`, `name[3]` for an array element, `name[3].inner` for a member of a
    /// structure element.
    pub member: String,
    /// The object the pointer refers to.
    pub target: Object<'t, 'a>,
}

impl<'t, 'a> Object<'t, 'a> {
    /// The object a value is.
    pub fn new(value: Value<'t, 'a>) -> Self {
        Self(value)
    }

    /// The underlying value.
    pub fn value(self) -> Value<'t, 'a> {
        self.0
    }

    /// The offset of the object in DATA: unique to the object in its file.
    pub fn offset(self) -> usize {
        self.0.offset
    }

    /// The name of the object's class as the tagfile spells it.
    pub fn class_name(self) -> &'t str {
        self.0.type_name()
    }

    /// The object's class, when it is one of [`Class`].
    pub fn class(self) -> Option<Class> {
        Class::from_name(self.class_name())
    }

    /// Whether the object's class is `class` or derives from it.
    pub fn is_a(self, class: &str) -> bool {
        is_a(self.0, class)
    }

    /// The view of the object's class `V`, when it is or derives from `V`'s class.
    pub fn view<V: View<'t, 'a>>(self) -> Option<V> {
        V::cast(self.0)
    }

    /// The object read as the view of its own class.
    pub fn typed(self) -> Option<Typed<'t, 'a>> {
        Typed::of(self.0)
    }

    /// Every pointer the object stores, in member order (inherited members first), with the members of structures
    /// and the elements of arrays of structures and pointers followed. Null pointers are left out.
    pub fn references(self) -> Result<Vec<Reference<'t, 'a>>> {
        let mut out = Vec::new();
        collect_references(self.0, "", &mut out, 0)?;
        Ok(out)
    }
}

fn join_path(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

fn collect_references<'t, 'a>(
    value: Value<'t, 'a>,
    path: &str,
    out: &mut Vec<Reference<'t, 'a>>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        return Err(invalid(value.at(), "structures nest more than 32 deep"));
    }
    match value.format()? & 15 {
        6 => {
            if let Some(target) = value.pointer()? {
                out.push(Reference {
                    member: path.to_owned(),
                    target: Object(target),
                });
            }
        }
        7 => {
            let types = &value.tag.types;
            let mut chain = Vec::new();
            let mut index = value.type_index;
            while index != 0 && chain.len() < 64 {
                let Some(t) = types.get(index) else {
                    break;
                };
                chain.push(t);
                index = t.parent;
            }
            for t in chain.into_iter().rev() {
                for field in &t.fields {
                    // bit 0: a member of the runtime object that is not stored
                    if field.flags & 1 != 0 {
                        continue;
                    }
                    let kind = value.tag.format(field.type_index)? & 15;
                    if !matches!(kind, 6..=8) {
                        continue;
                    }
                    collect_references(
                        value.field(&field.name)?,
                        &join_path(path, &field.name),
                        out,
                        depth.saturating_add(1),
                    )?;
                }
            }
        }
        8 => {
            let element = value.tag.inherited(value.type_index, |t| t.subtype)?;
            if !matches!(value.tag.format(element)? & 15, 6..=8) {
                return Ok(());
            }
            let array = value.array()?;
            for i in 0..array.len() {
                collect_references(
                    array.get(i)?,
                    &format!("{path}[{i}]"),
                    out,
                    depth.saturating_add(1),
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

// ---- counting

/// How many objects and structure elements of each class a tagfile (or a set of them) holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassCounts {
    /// Objects (what a pointer refers to, and the root), by class name.
    pub objects: BTreeMap<String, usize>,
    /// Elements of the runs of structures that arrays hold, by class name (arrays of numbers, characters and
    /// pointers are not counted).
    pub elements: BTreeMap<String, usize>,
}

impl ClassCounts {
    /// Add `other`'s counts to these.
    pub fn add(&mut self, other: &ClassCounts) {
        for (name, n) in &other.objects {
            let entry = self.objects.entry(name.clone()).or_default();
            *entry = entry.saturating_add(*n);
        }
        for (name, n) in &other.elements {
            let entry = self.elements.entry(name.clone()).or_default();
            *entry = entry.saturating_add(*n);
        }
    }

    /// The number of objects of the class named `name`.
    pub fn objects_of(&self, name: &str) -> usize {
        self.objects.get(name).copied().unwrap_or(0)
    }

    /// The number of structure elements of the class named `name`.
    pub fn elements_of(&self, name: &str) -> usize {
        self.elements.get(name).copied().unwrap_or(0)
    }
}

/// The objects and structure elements of each class in `tag`, from its item table.
pub fn class_counts(tag: &Tagfile<'_>) -> Result<ClassCounts> {
    let mut counts = ClassCounts::default();
    for item in tag.items.iter().skip(1) {
        let Some(t) = tag.types.get(item.type_index) else {
            continue;
        };
        let map = match item.kind {
            ItemKind::Object => &mut counts.objects,
            ItemKind::Run if tag.format(item.type_index)? & 15 == 7 => &mut counts.elements,
            _ => continue,
        };
        let entry = map.entry(t.name.clone()).or_default();
        *entry = entry.saturating_add(item.count);
    }
    Ok(counts)
}

// ---- documents

/// What a behaviour tagfile holds, from the class of the one variant of its root container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// An `hkbBehaviorGraph` with its data, string data and initial values and its nodes.
    Behavior,
    /// An `hkbCharacterData` with its string data and the values of the character properties.
    Character,
    /// An `hkbProjectData` with its string data.
    Project,
}

/// A behaviour tagfile read from its root.
#[derive(Clone, Copy, Debug)]
pub struct Document<'t, 'a> {
    tag: &'t Tagfile<'a>,
    kind: FileKind,
    main: Object<'t, 'a>,
}

impl<'t, 'a> Document<'t, 'a> {
    /// Classify `tag` by the one named variant of its root container. A file whose root is not an
    /// `hkRootLevelContainer` with one variant of a class of [`FileKind`] is refused.
    pub fn read(tag: &'t Tagfile<'a>) -> Result<Self> {
        let root = tag.root()?;
        if root.type_name() != "hkRootLevelContainer" {
            return Err(invalid(
                root.at(),
                format!("expected an hkRootLevelContainer, found a {}", root.type_name()),
            ));
        }
        let variants = root.field("namedVariants")?.array()?;
        if variants.len() != 1 {
            return Err(invalid(
                root.at(),
                format!(
                    "expected one named variant in the root container, found {}",
                    variants.len()
                ),
            ));
        }
        let variant = variants.get(0)?;
        let class_name = variant.field("className")?.string()?;
        let main = variant.field("variant")?.dereference()?;
        let kind = match class_name.as_str() {
            "hkbBehaviorGraph" => FileKind::Behavior,
            "hkbCharacterData" => FileKind::Character,
            "hkbProjectData" => FileKind::Project,
            other => {
                return Err(invalid(
                    variant.at(),
                    format!("the root variant is a {other}, not a behaviour graph, character or project"),
                ));
            }
        };
        if main.type_name() != class_name {
            return Err(invalid(
                main.at(),
                format!("the variant names {class_name} but is a {}", main.type_name()),
            ));
        }
        Ok(Self {
            tag,
            kind,
            main: Object(main),
        })
    }

    /// What the file is.
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    /// The tagfile.
    pub fn tagfile(&self) -> &'t Tagfile<'a> {
        self.tag
    }

    /// The object the root container's variant refers to (the graph, character data or project data).
    pub fn main(&self) -> Object<'t, 'a> {
        self.main
    }

    fn expect(&self, kind: FileKind) -> Result<()> {
        if self.kind == kind {
            Ok(())
        } else {
            Err(invalid(
                self.main.0.at(),
                format!("the file is a {:?} file, not a {kind:?} file", self.kind),
            ))
        }
    }

    /// The behaviour graph of a [`FileKind::Behavior`] file.
    pub fn behavior_graph(&self) -> Result<BehaviorGraph<'t, 'a>> {
        self.expect(FileKind::Behavior)?;
        Ok(BehaviorGraph::from_value(self.main.0))
    }

    /// The character data of a [`FileKind::Character`] file.
    pub fn character_data(&self) -> Result<CharacterData<'t, 'a>> {
        self.expect(FileKind::Character)?;
        Ok(CharacterData::from_value(self.main.0))
    }

    /// The project data of a [`FileKind::Project`] file.
    pub fn project_data(&self) -> Result<ProjectData<'t, 'a>> {
        self.expect(FileKind::Project)?;
        Ok(ProjectData::from_value(self.main.0))
    }

    /// Every object of the file (its object items, in item order), the root container first.
    pub fn objects(&self) -> Result<Vec<Object<'t, 'a>>> {
        let mut out = Vec::new();
        for (index, item) in self.tag.items.iter().enumerate().skip(1) {
            if item.kind == ItemKind::Object {
                out.push(Object(self.tag.item(index)?.get(0)?));
            }
        }
        Ok(out)
    }

    /// The objects reachable from the root container through pointers, each once, in the order a breadth-first walk
    /// finds them, the root container first.
    pub fn reachable(&self) -> Result<Vec<Object<'t, 'a>>> {
        let root = Object(self.tag.root()?);
        let mut seen = BTreeSet::from([root.offset()]);
        let mut order = vec![root];
        let mut queue = VecDeque::from([root]);
        while let Some(object) = queue.pop_front() {
            for reference in object.references()? {
                if seen.insert(reference.target.offset()) {
                    order.push(reference.target);
                    queue.push_back(reference.target);
                }
            }
        }
        Ok(order)
    }

    /// The objects and structure elements of each class.
    pub fn class_counts(&self) -> Result<ClassCounts> {
        class_counts(self.tag)
    }

    /// The character properties of a [`FileKind::Character`] file, with their names and values.
    pub fn character_properties(&self) -> Result<Vec<Variable<'t, 'a>>> {
        let data = self.character_data()?;
        let strings = data
            .string_data()?
            .ok_or_else(|| invalid(self.main.0.at(), "the character has no string data"))?;
        variables(
            self.main.0,
            data.character_property_infos()?,
            strings.character_property_names()?,
            None,
            data.character_property_values()?,
        )
    }

    /// The variables, events and variable bindings of a [`FileKind::Behavior`] file.
    pub fn behavior(&self) -> Result<Behavior<'t, 'a>> {
        let graph = self.behavior_graph()?;
        let missing = |what: &str| invalid(self.main.0.at(), format!("the behaviour graph has no {what}"));
        let data = graph.data()?.ok_or_else(|| missing("data"))?;
        let strings = data.string_data()?.ok_or_else(|| missing("string data"))?;
        Ok(Behavior { graph, data, strings })
    }
}

// ---- variables, events and bindings

/// The value a variable starts with (`hkbVariableValueSet`).
#[derive(Clone, Copy, Debug)]
pub enum Initial<'t, 'a> {
    /// A boolean, an integer or a real (`f32::from_bits` of the bits), as the word the set stores.
    Word(i32),
    /// A 3- or 4-component vector or a quaternion, from the set's quad values.
    Quad([f32; 4]),
    /// A pointer variable's object (usually none), from the set's variant values.
    Variant(Option<Object<'t, 'a>>),
}

/// A behaviour variable or character property with its name, type, role, bounds and initial value.
#[derive(Clone, Debug)]
pub struct Variable<'t, 'a> {
    /// The position among the variables (what a binding's `variableIndex` names).
    pub index: usize,
    /// The name from the string data.
    pub name: String,
    /// The type (`hkbVariableInfo::VariableType`).
    pub kind: EnumValue,
    /// The role (`hkbRoleAttribute::Role`): what the variable's value means to the engine.
    pub role: EnumValue,
    /// The role's flags (`hkbRoleAttribute::RoleFlags`).
    pub role_flags: EnumValue,
    /// The lower bound; `None` for a character property (a character stores no bounds).
    pub min: Option<i32>,
    /// The upper bound; `None` for a character property.
    pub max: Option<i32>,
    /// The initial value.
    pub initial: Initial<'t, 'a>,
}

impl Variable<'_, '_> {
    /// The initial value of a real variable (type `VARIABLE_TYPE_REAL`).
    pub fn initial_real(&self) -> Option<f32> {
        match (self.kind.raw, self.initial) {
            (VARIABLE_TYPE_REAL, Initial::Word(word)) => {
                Some(f32::from_bits(u32::from_ne_bytes(word.to_ne_bytes())))
            }
            _ => None,
        }
    }
}

/// `hkbVariableInfo::VariableType` values by which the initial values are laid out (HKLib's names; the census holds
/// them on every variable of both installs): the word value of a quad-typed variable is its index in the set's quad
/// values, and the word value of a pointer variable its index in the variant values.
const VARIABLE_TYPE_REAL: i64 = 4;
const VARIABLE_TYPE_POINTER: i64 = 5;
const VARIABLE_TYPES_QUAD: std::ops::RangeInclusive<i64> = 6..=8;

fn variables<'t, 'a>(
    at: Value<'t, 'a>,
    infos: Vec<VariableInfo<'t, 'a>>,
    names: Vec<String>,
    bounds: Option<Vec<VariableBounds<'t, 'a>>>,
    values: Option<VariableValueSet<'t, 'a>>,
) -> Result<Vec<Variable<'t, 'a>>> {
    if names.len() != infos.len() {
        return Err(invalid(
            at.at(),
            format!("{} variable infos but {} names", infos.len(), names.len()),
        ));
    }
    if let Some(bounds) = &bounds
        && bounds.len() != infos.len()
    {
        return Err(invalid(
            at.at(),
            format!("{} variable infos but {} bounds", infos.len(), bounds.len()),
        ));
    }
    let values = values.ok_or_else(|| invalid(at.at(), "no initial variable values"))?;
    let words = values.word_variable_values()?;
    let quads = values.quad_variable_values()?;
    let variants = values.variant_variable_values()?;
    if words.len() != infos.len() {
        return Err(invalid(
            at.at(),
            format!("{} variable infos but {} word values", infos.len(), words.len()),
        ));
    }
    let mut out = Vec::with_capacity(infos.len());
    for (index, (info, name)) in infos.iter().zip(names).enumerate() {
        let kind = info.r#type()?;
        let role = info.role()?;
        let word = words
            .get(index)
            .ok_or_else(|| invalid(at.at(), "a variable has no word value"))?
            .value()?;
        let slot = |len: usize, what: &str| {
            usize::try_from(word).ok().filter(|&i| i < len).ok_or_else(|| {
                invalid(
                    at.at(),
                    format!("variable {index} ({name}) names {what} {word}, past the {len} stored"),
                )
            })
        };
        let initial = if VARIABLE_TYPES_QUAD.contains(&kind.raw) {
            let i = slot(quads.len(), "quad value")?;
            Initial::Quad(
                quads
                    .get(i)
                    .copied()
                    .ok_or_else(|| invalid(at.at(), "quad value missing"))?,
            )
        } else if kind.raw == VARIABLE_TYPE_POINTER {
            let i = slot(variants.len(), "variant value")?;
            Initial::Variant(
                variants
                    .get(i)
                    .copied()
                    .ok_or_else(|| invalid(at.at(), "variant value missing"))?,
            )
        } else {
            Initial::Word(word)
        };
        let (min, max) = match bounds.as_ref().and_then(|b| b.get(index)) {
            Some(b) => (Some(b.min()?.value()?), Some(b.max()?.value()?)),
            None => (None, None),
        };
        out.push(Variable {
            index,
            name,
            kind,
            role: role.role()?,
            role_flags: role.flags()?,
            min,
            max,
            initial,
        });
    }
    Ok(out)
}

/// An event a behaviour graph declares.
#[derive(Clone, Debug)]
pub struct DeclaredEvent {
    /// The position among the events (what an `hkbEvent`'s `id` names).
    pub index: usize,
    /// The name from the string data.
    pub name: String,
    /// The flags (`hkbEventInfo::Flags`).
    pub flags: EnumValue,
}

/// What a variable binding names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingTarget {
    /// A variable of the behaviour graph (`BINDING_TYPE_VARIABLE`).
    Variable,
    /// A character property (`BINDING_TYPE_CHARACTER_PROPERTY`).
    CharacterProperty,
}

/// A member of a node that a variable drives.
#[derive(Clone, Debug)]
pub struct BoundMember {
    /// The member's path in the node (`hkbVariableBindingSet::Binding::memberPath`).
    pub member_path: String,
    /// Whether `index` names a variable or a character property.
    pub target: BindingTarget,
    /// The index among the graph's variables, or among its character properties.
    pub index: usize,
    /// The variable's or property's name.
    pub name: String,
    /// The bit of the variable that drives a boolean member, or negative for the whole value.
    pub bit_index: i8,
}

/// The counts a [`Behavior::check`] covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    /// The graph's variables.
    pub variables: usize,
    /// The graph's events.
    pub events: usize,
    /// The graph's character properties (the character file's properties as the graph declares them).
    pub character_properties: usize,
    /// Bindings to a variable, in every node's binding set.
    pub variable_bindings: usize,
    /// Bindings to a character property.
    pub property_bindings: usize,
}

/// A behaviour graph with its data and string data: the variables, events and bindings by name.
#[derive(Clone, Copy, Debug)]
pub struct Behavior<'t, 'a> {
    graph: BehaviorGraph<'t, 'a>,
    data: BehaviorGraphData<'t, 'a>,
    strings: BehaviorGraphStringData<'t, 'a>,
}

impl<'t, 'a> Behavior<'t, 'a> {
    /// The graph.
    pub fn graph(&self) -> BehaviorGraph<'t, 'a> {
        self.graph
    }

    /// The graph's data (variable infos and bounds, event infos, initial values).
    pub fn data(&self) -> BehaviorGraphData<'t, 'a> {
        self.data
    }

    /// The graph's string data (the names).
    pub fn string_data(&self) -> BehaviorGraphStringData<'t, 'a> {
        self.strings
    }

    /// The variables, with names, types, roles, bounds and initial values.
    pub fn variables(&self) -> Result<Vec<Variable<'t, 'a>>> {
        variables(
            self.graph.raw(),
            self.data.variable_infos()?,
            self.strings.variable_names()?,
            Some(self.data.variable_bounds()?),
            self.data.variable_initial_values()?,
        )
    }

    /// The character properties the graph declares (the same list the character file stores), without bounds and with
    /// no initial values of their own: the names and types.
    pub fn character_property_names(&self) -> Result<Vec<String>> {
        self.strings.character_property_names()
    }

    /// The events, with names and flags.
    pub fn events(&self) -> Result<Vec<DeclaredEvent>> {
        let names = self.strings.event_names()?;
        let infos = self.data.event_infos()?;
        if names.len() != infos.len() {
            return Err(invalid(
                self.graph.raw().at(),
                format!("{} event infos but {} names", infos.len(), names.len()),
            ));
        }
        let mut out = Vec::with_capacity(infos.len());
        for (index, (info, name)) in infos.iter().zip(names).enumerate() {
            out.push(DeclaredEvent {
                index,
                name,
                flags: info.flags()?,
            });
        }
        Ok(out)
    }

    /// The members of `node` that variables drive, with the variables' names. A node without a binding set has none.
    pub fn bindings(&self, node: Object<'t, 'a>) -> Result<Vec<BoundMember>> {
        if !node.value().has_field("variableBindingSet") {
            return Ok(Vec::new());
        }
        match pointer_view::<VariableBindingSet>(node.value().field("variableBindingSet")?)? {
            Some(set) => self.set_bindings(set),
            None => Ok(Vec::new()),
        }
    }

    /// The members a binding set drives, with the variables' names.
    pub fn set_bindings(&self, set: VariableBindingSet<'t, 'a>) -> Result<Vec<BoundMember>> {
        let variable_names = self.strings.variable_names()?;
        let property_names = self.strings.character_property_names()?;
        let mut out = Vec::new();
        for binding in set.bindings()? {
            let raw = binding.variable_index()?;
            let index = usize::try_from(raw).map_err(|_| {
                invalid(
                    binding.raw().at(),
                    format!("a binding names variable {raw}, which is negative"),
                )
            })?;
            let type_kind = binding.binding_type()?;
            let (target, names) = match type_kind.name() {
                Some("BINDING_TYPE_VARIABLE") => (BindingTarget::Variable, &variable_names),
                Some("BINDING_TYPE_CHARACTER_PROPERTY") => {
                    (BindingTarget::CharacterProperty, &property_names)
                }
                _ => {
                    return Err(invalid(
                        binding.raw().at(),
                        format!(
                            "a binding has the type {}, which is neither a variable nor a character property",
                            type_kind.raw
                        ),
                    ));
                }
            };
            let name = names.get(index).ok_or_else(|| {
                invalid(
                    binding.raw().at(),
                    format!(
                        "a binding names {target:?} {index}, past the {} declared",
                        names.len()
                    ),
                )
            })?;
            out.push(BoundMember {
                member_path: binding.member_path()?,
                target,
                index,
                name: name.clone(),
                bit_index: binding.bit_index()?,
            });
        }
        Ok(out)
    }

    /// Check what ties the graph's tables together, over every binding set of `document`: as many variable infos as bounds and
    /// names, an initial word for each variable and quad and variant values for the quad- and pointer-typed ones,
    /// as many event infos as names, and every binding naming a variable or character property that exists. Returns
    /// the counts covered; the first inconsistency is the error.
    pub fn check(&self, document: &Document<'t, 'a>) -> Result<Checked> {
        let variables = self.variables()?;
        let events = self.events()?;
        let properties = self.strings.character_property_names()?;
        if self.data.character_property_infos()?.len() != properties.len() {
            return Err(invalid(
                self.graph.raw().at(),
                "the graph's character property infos and names differ in number",
            ));
        }
        if self.data.attribute_defaults()?.len() != self.strings.attribute_names()?.len() {
            return Err(invalid(
                self.graph.raw().at(),
                "the graph's attribute defaults and names differ in number",
            ));
        }
        if self.graph.root_generator()?.is_none() {
            return Err(invalid(
                self.graph.raw().at(),
                "the behaviour graph has no root generator",
            ));
        }
        let mut checked = Checked {
            variables: variables.len(),
            events: events.len(),
            character_properties: properties.len(),
            ..Checked::default()
        };
        for object in document.objects()? {
            let Some(set) = object.view::<VariableBindingSet>() else {
                continue;
            };
            for bound in self.set_bindings(set)? {
                let n = match bound.target {
                    BindingTarget::Variable => &mut checked.variable_bindings,
                    BindingTarget::CharacterProperty => &mut checked.property_bindings,
                };
                *n = n.saturating_add(1);
            }
        }
        Ok(checked)
    }
}

#[cfg(test)]
mod tests {
    use super::enums::{
        HKB_BLENDING_TRANSITION_EFFECT_FLAG_BITS, HKB_CLIP_GENERATOR_PLAYBACK_MODE,
        HKB_VARIABLE_INFO_VARIABLE_TYPE,
    };
    use super::*;

    // The expected member names and values are HKLib's (hk2018 classes), read from its sources.
    #[test]
    fn an_enumeration_names_its_values_and_a_flag_set_its_bits() {
        assert_eq!(HKB_CLIP_GENERATOR_PLAYBACK_MODE.name_of(1), Some("MODE_LOOPING"));
        assert_eq!(HKB_CLIP_GENERATOR_PLAYBACK_MODE.name_of(99), None);
        assert_eq!(
            HKB_VARIABLE_INFO_VARIABLE_TYPE.name_of(-1),
            Some("VARIABLE_TYPE_INVALID")
        );
        let (names, rest) = HKB_BLENDING_TRANSITION_EFFECT_FLAG_BITS.flag_names(2 | 16 | 64);
        assert_eq!(names, ["FLAG_SYNC", "FLAG_DONT_BLEND_CONTROLS_DATA"]);
        assert_eq!(rest, 64);
    }

    #[test]
    fn a_stored_value_knows_whether_its_table_names_it() {
        let mode = |raw| EnumValue {
            raw,
            info: Some(&HKB_CLIP_GENERATOR_PLAYBACK_MODE),
        };
        assert_eq!(mode(1).name(), Some("MODE_LOOPING"));
        assert!(mode(1).is_named());
        assert!(!mode(9).is_named());
        let flags = |raw| EnumValue {
            raw,
            info: Some(&HKB_BLENDING_TRANSITION_EFFECT_FLAG_BITS),
        };
        assert!(flags(0).is_named());
        assert!(flags(2 | 4).is_named());
        assert!(!flags(2 | 64).is_named());
        assert_eq!(flags(2 | 64).flag_names(), ["FLAG_SYNC"]);
        let without_table = EnumValue { raw: 0, info: None };
        assert!(!without_table.is_named());
        assert!(without_table.flag_names().is_empty());
    }

    #[test]
    fn every_class_and_table_is_found_by_its_tagfile_name() {
        assert_eq!(Class::ALL.len(), 93);
        for &class in Class::ALL {
            assert_eq!(Class::from_name(class.name()), Some(class));
        }
        assert_eq!(
            Class::from_name("hkbStateMachine::StateInfo"),
            Some(Class::StateMachineStateInfo)
        );
        assert_eq!(Class::StateMachineStateInfo.parent(), Some(Class::Bindable));
        assert_eq!(Class::from_name("hkaSkeleton"), None);
        assert_eq!(enums::ALL.len(), 41);
        for &info in enums::ALL {
            assert!(std::ptr::eq(
                enum_info(info.name).expect("a table by its own name"),
                info
            ));
        }
        assert!(enum_info("hkbNoSuchEnum").is_none());
    }

    #[test]
    fn class_counts_add_up() {
        let mut total = ClassCounts::default();
        let mut one = ClassCounts::default();
        one.objects.insert("hkbClipGenerator".into(), 2);
        one.elements.insert("hkbEventInfo".into(), 5);
        total.add(&one);
        total.add(&one);
        assert_eq!(total.objects_of("hkbClipGenerator"), 4);
        assert_eq!(total.elements_of("hkbEventInfo"), 10);
        assert_eq!(total.objects_of("hkbStateMachine"), 0);
    }
}
