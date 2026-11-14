# havok-formats

[![crates.io: havok-formats](https://img.shields.io/crates/v/havok-formats?label=havok-formats)](https://crates.io/crates/havok-formats) [![docs.rs](https://img.shields.io/docsrs/havok-formats)](https://docs.rs/havok-formats) [![CI](https://img.shields.io/github/actions/workflow/status/Kalmarv/havok-formats/ci.yml?branch=main&label=CI)](https://github.com/Kalmarv/havok-formats/actions/workflows/ci.yml)

A Rust library that reads and writes Havok physics and animation files (`.hkx` tagfiles and packfiles) as games ship them. It was written against the files of Sekiro, Elden Ring and Tony Hawk's SHRED (PS3), and contains no Havok SDK code.

## Features

- Reads tagfiles (`TAG0`) of SDK 2015.1, 2016.2 and 2018.1 and their shared type compendia (`TCM0`), with any class readable field by field from the file's own type table
- Reads packfiles of version 7 (`Havok-7.1.0-r1`, big endian) and versions 9 and 11 (`hk_2013.1.0-r1`, `hk_2014.1.0-r1`), and the class metadata some of them carry
- Writes tagfiles and packfiles back, byte for byte when unedited
- Decodes `hknp` compressed collision meshes with their bounding-volume trees, extern triangle meshes and convex shapes
- Builds compressed and extern collision meshes from triangles and writes them into a tagfile
- Reads cloth data (`hclClothData`), ragdolls and the typed atoms of ragdoll, hinge, ball and socket, fixed and wheel constraints
- Reads skeletons, animation bindings and animation containers
- Decodes, samples and encodes `hkaSplineCompressedAnimation`, and writes an animation back into its file
- Reads Havok AI navigation meshes, Sekiro's cluster graphs and Elden Ring's query trees, and triangulates their faces
- Reads Havok Behavior files through typed views of the 93 `hkb*` and `Custom*` classes in Sekiro's and Elden Ring's behaviour binders
- Reads Havok 7.1 `hkp` rigid bodies, collision shapes, constraints and MOPP code from packfiles
- Writes a tagfile's objects as Havok's XML (`hktagfile` version 3) and lists the differences between two readings
- Names the enumerated values and flags the files store as numbers
- Bounds-checks every read: bad input gives an `Error` naming the byte offset, never a panic
- Refuses unknown versions, sections, flags and classes by name instead of guessing

## Getting Started

### Requirements

Rust 1.99 or later (edition 2024). The library takes bytes, not paths, so reading files and archives is up to the caller.

### Installation

```sh
cargo add havok-formats
```

### Usage

Print the triangle count of each collision mesh in a Sekiro map collision tagfile whose types are in a shared compendium:

```rust
use havok_formats::{Compendium, Tagfile, compressed_mesh};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let compendium = Compendium::read(&std::fs::read("shared.compendium")?)?;
    let bytes = std::fs::read("collision.hkx")?;
    let tagfile = Tagfile::read(&bytes, Some(&compendium))?;

    let variants = tagfile.root()?.field("namedVariants")?.array()?;
    for i in 0..variants.len() {
        let physics = variants.get(i)?.field("variant")?.dereference()?;
        let systems = physics.field("systemDatas")?.array()?;
        for j in 0..systems.len() {
            let bodies = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
            for k in 0..bodies.len() {
                let shape = bodies.get(k)?.field("shape")?.dereference()?;
                let mesh = compressed_mesh(shape.field("data")?.dereference()?)?;
                println!("{}: {} triangles", shape.type_name(), mesh.triangles.len());
            }
        }
    }
    Ok(())
}
```

A file the reader refuses says where and why:

```rust
let error = havok_formats::Tagfile::read(b"\0\0\0\x07TAG0", None).unwrap_err();
assert_eq!(error.to_string(), "Havok at 0x0: section length is less than its header");
```

The same program ships as an example, taking its paths from the command line:

```sh
cargo run --example triangles -- collision.hkx shared.compendium
```

Other entry points: `animation::container` and `spline::SplineAnimation` for animation, `cloth::read` for cloth and ragdolls, `navmesh::nav_data` for navigation meshes, `behavior::Document` for behaviour files, `packfile::Packfile` for packfiles, and `xml::objects` for the XML form. The [API documentation](https://docs.rs/havok-formats) covers every module.

`cargo test --workspace` runs the unit tests, the hand-built fixtures and the doc tests without any game files. The real-data tests read your own copies of game files through the `GAME_DATA` configuration described in [primase-testdata](https://github.com/Kalmarv/primase-testdata), and skip when it is not set; [tests/inputs/README.md](tests/inputs/README.md) lists each input. No game files are included in the repository.

See [CONTRIBUTING.md](https://github.com/Kalmarv/havok-formats/blob/fa26978/CONTRIBUTING.md) for the checks CI runs and how to report a file the reader cannot read. Licensed under GPL-3.0-or-later.

## Changes in this copy

This is a copy of [Kalmarv/havok-formats](https://github.com/Kalmarv/havok-formats) at commit `fa26978` (0.10.0),
changed for Hookshot Mod Studio (2026-11-13), which reads the cloth files (`.bphcl`) of The Legend of Zelda: Ocarina
of Time for Switch 2. Those are tagfiles of SDK 2025.2. Changes, each commented where it is made:

- `Cargo.toml`: the package settings its workspace held are written out (the copy is a member of Mod Studio's
  workspace); not published. This README links to the original's `CONTRIBUTING.md`.
