//! For every model file: its size, the game's resource size table value, and what it holds (GPU data, bones,
//! shapes, meshes, materials, ...), as CSV, to work out what the table's extra over the file size depends on
//! (local analysis for the size table, docs/plan.md phase 3).
//!
//! `cargo run --release --example restbl_fit -- <romfs> <decompressed table> > models.csv`

use std::collections::HashMap;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::restbl::crc32;
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, table] = args.as_slice() else {
        eprintln!("usage: restbl_fit <romfs> <table>");
        std::process::exit(2);
    };
    let t = std::fs::read(table).unwrap();
    let u32_at = |at: usize| u32::from_le_bytes(t[at..at + 4].try_into().unwrap());
    let ncrc = u32_at(14) as usize;
    let by_crc: HashMap<u32, u32> = (0..ncrc)
        .map(|i| (u32_at(22 + i * 8), u32_at(26 + i * 8)))
        .collect();
    let romfs = Romfs::open(root).unwrap();
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    let mut files: Vec<String> = std::fs::read_dir(std::path::Path::new(root).join("Model"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".bfres.zs") && !f.contains(".anim."))
        .collect();
    files.sort();
    println!(
        "file,size,table,gpu,index,vertex,models,bones,smooth,rigid,shapes,meshes,submeshes,meshlet_shapes,\
         materials,textures,samplers,params,param_bytes,render_info,vbufs,attrs,buffers,vertices,skin_bones"
    );
    for f in files {
        let path = format!("Model/{}", f.trim_end_matches(".zs"));
        let Some(&table) = by_crc.get(&crc32(path.as_bytes())) else {
            continue;
        };
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        let mut c = [0usize; 22];
        c[0] = data.len() - b.buffer_offset.min(data.len());
        for m in &b.models {
            c[3] += 1;
            c[4] += m.skeleton.bones.len();
            c[5] += m.skeleton.smooth_count;
            c[6] += m.skeleton.rigid_count;
            for s in &m.shapes {
                c[7] += 1;
                c[8] += s.meshes.len();
                c[10] += usize::from(s.meshes.is_empty());
                c[21] += s.skin_bone_indices.len();
                for mesh in &s.meshes {
                    c[9] += mesh.sub_meshes.len();
                    c[1] += mesh.index_data.len();
                }
            }
            for mat in &m.materials {
                c[11] += 1;
                c[12] += mat.textures.len();
                c[13] += mat.samplers.len();
                if let Some(sh) = &mat.shader {
                    c[14] += sh.params.len();
                    c[15] += sh.params.iter().map(|p| p.data.len()).sum::<usize>();
                    c[16] += sh.render_info.len();
                }
            }
            for vb in &m.vertex_buffers {
                c[17] += 1;
                c[18] += vb.attributes.len();
                c[19] += vb.buffers.len();
                c[20] += vb.vertex_count as usize;
                c[2] += vb.buffers.iter().map(|x| x.data.len()).sum::<usize>();
            }
        }
        let cols: Vec<String> = c.iter().map(|v| v.to_string()).collect();
        println!("{f},{},{table},{}", data.len(), cols.join(","));
    }
}
