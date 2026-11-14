//! Prints the compressed collision meshes of a Havok tagfile.
//!
//! `cargo run --example triangles -- collision.hkx [shared.compendium]`
//!
//! The file must be a tagfile whose root holds physics data (as Sekiro's map collision files do). Give the compendium
//! when the file's types are stored outside it.

use havok_formats::{Compendium, Tagfile, compressed_mesh};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: triangles <file.hkx> [<file.compendium>]");
        std::process::exit(2);
    };
    let compendium = match args.next() {
        Some(p) => Some(Compendium::read(&std::fs::read(p)?)?),
        None => None,
    };
    let bytes = std::fs::read(path)?;
    let tagfile = Tagfile::read(&bytes, compendium.as_ref())?;
    println!(
        "SDK {}, {} types, {} items",
        String::from_utf8_lossy(&tagfile.version),
        tagfile.types.len().saturating_sub(1),
        tagfile.items.len().saturating_sub(1)
    );

    let variants = tagfile.root()?.field("namedVariants")?.array()?;
    for i in 0..variants.len() {
        let physics = variants.get(i)?.field("variant")?.dereference()?;
        let systems = physics.field("systemDatas")?.array()?;
        for j in 0..systems.len() {
            let bodies = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
            for k in 0..bodies.len() {
                let shape = bodies.get(k)?.field("shape")?.dereference()?;
                let mesh = compressed_mesh(shape.field("data")?.dereference()?)?;
                println!(
                    "{}: {} triangles, {} dead, {} custom",
                    shape.type_name(),
                    mesh.triangles.len(),
                    mesh.dead_primitives,
                    mesh.custom.len()
                );
            }
        }
    }
    Ok(())
}
