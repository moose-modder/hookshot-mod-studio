//! Reads Havok physics and animation data as games store it: tagfiles (`.hkx`) and packfiles, and the collision meshes,
//! convex shapes, cloth, ragdolls, skeletons and spline-compressed animations inside them. Every read is
//! bounds-checked, so untrusted files produce errors, not panics. It is an independent reader of the stored formats and
//! contains no Havok SDK code.
//!
//! # What it reads
//!
//! - **Tagfiles** (`TAG0`, SDK versions 2015.1, 2016.2 and 2018.1, see [`SDK_VERSIONS`]) and their external type compendia
//!   (`TCM0`): [`Tagfile`] and [`Compendium`]. Class layouts come from the file's own type metadata, so any class
//!   can be read field by field through [`Value`] and [`Array`]. [`Tagfile::image`] makes an editable [`TagfileImage`],
//!   which writes a tagfile again.
//! - **Packfiles** (version 7, contents version `Havok-7.1.0-r1`, 4-byte pointers, big endian; versions 9 and 11,
//!   `hk_2013.1.0-r1` and `hk_2014.1.0-r1`, 8-byte pointers, little endian): [`packfile`], their editable image
//!   ([`packfile::image`]) and the class metadata some of them carry ([`packfile::reflection`]).
//! - Behaviour tagfiles of Sekiro and Elden Ring (`hkb*` and `Custom*` classes): typed views of every class, and a reader
//!   of the behaviour graphs, characters and projects they hold ([`behavior`]).
//! - A tagfile's objects as the XML Havok's tools exchange, an XML parser and writer for that subset and a difference
//!   listing, to compare this crate's reading of a file with another tool's ([`xml`]).
//! - Objects read from them: `hknp` compressed meshes ([`compressed_mesh`], with their bounding-volume trees
//!   [`mesh_trees`] and [`simd_tree`]), extern triangle meshes ([`extern_mesh`]),
//!   convex shapes ([`convex`]), cloth and ragdoll data ([`cloth`]), skeletons, animation bindings and spline
//!   compressed animation ([`animation`], [`spline`]), navigation meshes, their cluster graphs and query trees
//!   ([`navmesh`]), the typed atoms of ragdoll and other constraints ([`constraint`]), and the 7.1 `hkp` rigid bodies
//!   and collision shapes of packfiles ([`packfile::physics`]) with the
//!   MOPP code that indexes their triangles ([`mopp`]). The convex point sets of custom mesh primitives are decoded
//!   ([`PointSet`]).
//! - Collision written from triangles: [`build_compressed_mesh`] makes the mesh tree arrays of an `hknp` compressed mesh
//!   ([`BuiltMesh`]: sections, packed and shared vertices, section and mesh trees, the simd tree, the interior
//!   flags, [`interior_keys`]) and
//!   [`replace_compressed_mesh`] puts them into the shape of a [`TagfileImage`]; [`replace_extern_mesh`] does the same
//!   for an extern mesh. The layouts come from the file's own type table.
//! - Names for the enumerated members the files store as numbers ([`enums`]: `hkp` motion, response, broad-phase,
//!   welding, striding, subpart and MOPP build types; `hka` blend hints and reference
//!   frames; `hknp` motion and shape types; `hcl` shape types and platforms), the member declaration flags
//!   ([`DeclFlags`]: who may reach a member, whether it is written) and the mesh-tree section flags
//!   ([`SectionFlags`]).
//!
//! It was written against Sekiro (tagfiles, 2016.2; packfiles, versions 9 and 11), Elden Ring (tagfiles, 2018.1;
//! packfiles, versions 9 and 11) and Tony Hawk's SHRED for PS3 (packfiles, version 7).
//!
//! # Guarantees
//!
//! Every read is checked. Data that is shorter than it says, refers outside itself, or holds something the reader
//! does not know is an [`Error`] that names the byte offset and what was wrong, never a panic. Unknown sections,
//! versions and flags are refused rather than skipped. Type indices are one-based; zero denotes a null type or object
//! reference.
//!
//! It does not validate every byte of a file, only what the accessors you call read. It writes tagfiles
//! ([`Tagfile::image`] and [`TagfileImage::write`]: an unedited file comes back byte for byte, an edited one with its
//! items numbered and laid out as the game's files are) and packfiles
//! ([`packfile::Packfile::image`] and [`packfile::image::PackfileImage::write`], the same), and it does not simulate
//! anything.
//!
//! # Example
//!
//! Read the triangles of the collision meshes in a tagfile (a Sekiro map collision file, whose types are in a shared
//! compendium):
//!
//! ```no_run
//! use havok_formats::{Compendium, Tagfile, compressed_mesh};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let compendium = Compendium::read(&std::fs::read("shared.compendium")?)?;
//!     let bytes = std::fs::read("collision.hkx")?;
//!     let tagfile = Tagfile::read(&bytes, Some(&compendium))?;
//!
//!     let variants = tagfile.root()?.field("namedVariants")?.array()?;
//!     for i in 0..variants.len() {
//!         let physics = variants.get(i)?.field("variant")?.dereference()?;
//!         let systems = physics.field("systemDatas")?.array()?;
//!         for j in 0..systems.len() {
//!             let bodies = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
//!             for k in 0..bodies.len() {
//!                 let shape = bodies.get(k)?.field("shape")?.dereference()?;
//!                 let mesh = compressed_mesh(shape.field("data")?.dereference()?)?;
//!                 println!("{}: {} triangles", shape.type_name(), mesh.triangles.len());
//!             }
//!         }
//!     }
//!     Ok(())
//! }
//! ```
//!
//! A file the reader refuses says where and why:
//!
//! ```
//! let error = havok_formats::Tagfile::read(b"\0\0\0\x07TAG0", None).unwrap_err();
//! assert_eq!(error.offset, 0);
//! assert_eq!(error.to_string(), "Havok at 0x0: section length is less than its header");
//! ```
#![deny(missing_docs, clippy::disallowed_methods, clippy::disallowed_types)]
#![cfg_attr(
    not(test),
    deny(
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::indexing_slicing
    )
)]

#[cfg(test)]
#[path = "../tests/inputs/mod.rs"]
mod inputs;

pub mod animation;
mod animation_write;
pub mod behavior;
mod bvtree;
mod bvtree_write;
pub mod cloth;
pub mod constraint;
pub mod convex;
mod degenerate;
pub mod enums;
mod extern_mesh;
mod extern_write;
mod gamesort;
mod interior;
mod mesh;
pub mod navmesh;

pub mod mopp;
pub mod packfile;
mod polygon;

mod mesh_write;

mod quads;
mod schema;
mod section;
mod simd_write;
mod simdtree;
pub mod spline;
mod spline_data;
mod spline_encode;
mod spline_fit;
mod spline_sample;
mod stock_tree;

pub use bvtree::{
    BOUNDS_TOLERANCE_ULPS, BoundsCheck, BvNode, BvNodeKind, BvTree, MeshTrees, NodeWidth, SectionTree,
    mesh_trees,
};
pub use bvtree_write::{
    Aabb, EncodedTree, MERGE_LEAVES, Shape, balanced_shape, encode_tree, sah_shape, sah_shape_with, shape_of,
};
pub use degenerate::{DEFAULT_DEGENERATE_THRESHOLD, is_degenerate_triangle};
pub use extern_mesh::{ExternMesh, extern_mesh};
pub use extern_write::{ExternSource, extern_mesh_shapes, replace_extern_mesh};
pub use gamesort::game_sort_by;
pub use interior::{
    InteriorPrimitive, MeshView, SectionView, game_sort, interior_keys, primitives_of, rsqrt_estimate,
};
pub use mesh::{
    CompressedMesh, CustomKind, CustomPrimitive, MeshSection, PointSet, SectionFlags, ShapeInstance,
    Triangle, compressed_mesh, extern_shapes, interior_primitives, mesh_sections,
};
pub use mesh_write::{
    BuildOptions, BuiltMesh, BuiltSection, Layout, MAX_PRIMITIVES, MAX_SLOTS, MeshSource, Primitives,
    build_compressed_mesh, build_compressed_mesh_with, compressed_mesh_shapes, replace_compressed_mesh,
};
pub use quads::{QuadCandidate, pair_quads};
pub use simd_write::{EMPTY_BOX, SimdEncoding, SimdLeaf, build_simd_tree};
pub use stock_tree::{Box3, StockTree, section_numbering};
mod tagfile;
mod tagfile_write;
mod types;
mod value;
pub mod xml;
pub use tagfile::{Item, ItemKind, SDK_VERSIONS, Tagfile};
pub use tagfile_write::{ImageItem, TagfileImage};
pub use types::{Compendium, DeclAccess, DeclFlags, Field, Type, TypeFlags};
pub use value::{Array, HALF_FORMAT, Value};

pub use section::{Section, sections};
pub use simdtree::{LaneBox, SimdCheck, SimdLane, SimdNode, SimdTree, simd_tree};

/// A refusal: what is wrong, at the byte offset where it was found.
#[derive(Debug, thiserror::Error)]
#[error("Havok at {offset:#x}: {detail}")]
pub struct Error {
    /// The byte offset in the data read.
    pub offset: usize,
    /// What the data holds that the reader refuses, or lacks.
    pub detail: String,
}

impl Error {
    /// A refusal at `offset`.
    pub fn new(offset: usize, detail: impl Into<String>) -> Self {
        Error {
            offset,
            detail: detail.into(),
        }
    }
}

/// A read's result.
pub type Result<T> = std::result::Result<T, Error>;

/// A failed byte-range read of a cursor, as a refusal at the range's offset in the data read.
impl From<primase_foundations::bytes::BoundsError> for Error {
    fn from(e: primase_foundations::bytes::BoundsError) -> Self {
        use primase_foundations::bytes::BoundsError;
        let detail = match e {
            BoundsError::LengthOverflow { count, stride, .. } => {
                format!("{count} entries of {stride} bytes overflow the address space")
            }
            BoundsError::OutOfBounds { length, total, .. } => {
                format!("{length} bytes run past the end of the {total} bytes of data")
            }
        };
        invalid(usize::try_from(e.absolute()).unwrap_or(usize::MAX), detail)
    }
}

fn invalid(offset: usize, detail: impl Into<String>) -> Error {
    Error::new(offset, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_foundations::bytes::BoundsError;

    #[test]
    fn a_bounds_error_is_a_refusal_at_its_offset_that_says_what_ran_out() {
        let past = Error::from(BoundsError::OutOfBounds {
            offset: 4,
            origin: 0x10,
            length: 8,
            total: 6,
        });
        assert_eq!(past.offset, 0x14);
        assert_eq!(past.detail, "8 bytes run past the end of the 6 bytes of data");
        let overflow = Error::from(BoundsError::LengthOverflow {
            offset: 4,
            origin: 0x10,
            count: usize::MAX,
            stride: 4,
        });
        assert_eq!(overflow.offset, 0x14);
        assert!(overflow.detail.contains("overflow"), "{overflow}");
        assert!(!past.detail.contains("animation") && !past.detail.contains("block"));
    }
}
