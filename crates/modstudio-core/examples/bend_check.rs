//! How the game's cloth parts build their bend stiffness (`hclBendStiffnessConstraintSet`), checked on every cloth
//! file, for what `cloth_new::bend_link` makes of it. Each link is two triangles sharing an edge: particles A, B, C, D,
//! C-D the edge, A and B the far corners. From the sim cloth's rest pose:
//! - the weights `[a, b, -(1 - t), -t]`: `a` and `b` the far corners' heights over the edge, each as a share of the
//!   two's (B's height for A), so `a A + b B` is where A to B crosses the edge with the triangles flattened, and `t`
//!   where that point falls along C-D. A and B: as stored, on every link. C and D: as stored where the triangles lie
//!   in one plane, and further off the more they are folded (reported by fold; pairs with a far corner beyond the
//!   edge's ends left out, where a few thin triangles' differ);
//! - the stiffness `-beta / (the four particles' inverse masses, summed)`, `beta` one value a part (mostly);
//! - the rest curvature: what makes the bending vector nothing at rest (`havok-cloth::constraints::bend_stiffness`,
//!   rest pose used);
//! - `maxRestPoseHeightSq`: against the set's largest height at rest (the rest curvature times `current`), squared;
//! - which pairs of triangles get a link, and the sets' flags.
//!
//! ```text
//! cargo run --release -p modstudio-core --example bend_check -- <romfs> [part of a file name]
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use havok_formats::Tagfile;
use havok_formats::cloth::{self, ConstraintKind};
use modstudio_formats::phive::Phive;
use modstudio_formats::romfs::Romfs;

type V = [f64; 3];
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: V) -> f64 {
    dot(a, a).sqrt()
}

/// The weights as `cloth_new::bend_link` makes them.
fn weights(a: V, b: V, c: V, d: V) -> [f64; 4] {
    let cd = sub(d, c);
    let edge = len(cd).max(1e-12);
    let ha = len(cross(cd, sub(a, c))) / edge;
    let hb = len(cross(cd, sub(b, c))) / edge;
    let wa = if ha + hb > 0.0 { hb / (ha + hb) } else { 0.5 };
    let p = [0, 1, 2].map(|k| wa * a[k] + (1.0 - wa) * b[k]);
    let t = dot(sub(p, c), cd) / (edge * edge);
    [wa, 1.0 - wa, -(1.0 - t), -t]
}

/// The rest curvature that cancels the bending vector `bv` at rest as the simulator measures a bend (its offset, the
/// two triangles' normals summed; and `current`), and `current`.
fn rest_curvature(p: [V; 4], w: [f64; 4]) -> (f64, f64) {
    let [pa, pb, pc, pd] = p;
    let bv = [0, 1, 2].map(|k| w[0] * pa[k] + w[1] * pb[k] + w[2] * pc[k] + w[3] * pd[k]);
    let cd = sub(pd, pc);
    let x = cross(cd, sub(pa, pc));
    let y = cross(sub(pb, pc), cd);
    let (lx, ly) = (len(x), len(y));
    let o = [0, 1, 2].map(|k| x[k] / lx + y[k] / ly);
    let current = lx * ly / dot(cd, cd);
    (-dot(bv, o) / len(o) / current, current)
}

fn quantile(v: &mut [f64], f: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(((v.len().max(1) - 1) as f64 * f) as usize)
        .copied()
        .unwrap_or(0.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: bend_check <romfs> [part of a file name]");
        std::process::exit(2);
    };
    let filter = args.get(1).cloned().unwrap_or_default();
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(root).join("Phive/Cloth"))
        .unwrap()
        .flatten()
        .map(|e| format!("Phive/Cloth/{}", e.file_name().to_string_lossy()))
        .filter(|f| f.contains(filter.as_str()))
        .collect();
    files.sort();
    let (mut links_n, mut with_set) = (0usize, 0usize);
    let mut ab_worst = 0f64;
    // C and D, by how far the two triangles are folded (under 1, under 10, under 45 degrees, more): their errors;
    // pairs whose far corners both lie between the edge's ends only
    let mut by_fold: Vec<Vec<f64>> = vec![Vec::new(); 4];
    let mut beyond = 0usize;
    let mut curvature_rel: Vec<f64> = Vec::new();
    let mut betas: BTreeMap<String, usize> = BTreeMap::new();
    let mut beta_mixed = 0usize;
    let mut flags: BTreeMap<String, usize> = BTreeMap::new();
    let mut height_ratio: Vec<f64> = Vec::new();
    let (mut pairs_all, mut pairs_linked, mut links_elsewhere) = (0usize, 0usize, 0usize);
    for f in &files {
        let bytes = romfs.read(&mut d, f).unwrap();
        let phive = Phive::parse(&bytes).unwrap();
        let tag = Tagfile::read(phive.tagfile, None).unwrap();
        let physics = cloth::read(&tag).unwrap();
        let mut has = false;
        for c in &physics.cloths {
            for sc in &c.sim_cloths {
                let Some(pose) = sc.poses.first() else {
                    continue;
                };
                let pos = |i: u16| -> V {
                    std::array::from_fn(|k| f64::from(pose.positions[usize::from(i)][k]))
                };
                let inv = |i: u16| f64::from(sc.particles[usize::from(i)].inv_mass);
                // every pair of triangles sharing an edge: (the edge, the far corners), each sorted
                let mut by_edge: BTreeMap<(u16, u16), Vec<u16>> = BTreeMap::new();
                for t in sc.triangle_indices.chunks(3) {
                    for k in 0..3 {
                        let (a, b, o) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                        by_edge.entry((a.min(b), a.max(b))).or_default().push(o);
                    }
                }
                let mut tri_pairs: BTreeSet<((u16, u16), (u16, u16))> = BTreeSet::new();
                for (e, far) in &by_edge {
                    for i in 0..far.len() {
                        for j in i + 1..far.len() {
                            tri_pairs.insert((*e, (far[i].min(far[j]), far[i].max(far[j]))));
                        }
                    }
                }
                for set in &sc.constraint_sets {
                    let ConstraintKind::BendStiffness {
                        links,
                        max_rest_pose_height_sq,
                        clamp_bend_stiffness,
                        use_rest_pose_config,
                    } = &set.kind
                    else {
                        continue;
                    };
                    has = true;
                    *flags
                        .entry(format!(
                            "rest pose {use_rest_pose_config}, clamp {clamp_bend_stiffness}"
                        ))
                        .or_default() += 1;
                    let mut part_betas = Vec::new();
                    let mut most_height = 0f64;
                    let mut linked: BTreeSet<((u16, u16), (u16, u16))> = BTreeSet::new();
                    for l in links {
                        links_n += 1;
                        let [a, b, c2, d2] = l.particles;
                        linked.insert(((c2.min(d2), c2.max(d2)), (a.min(b), a.max(b))));
                        let stored: [f64; 4] = l.weights.map(f64::from);
                        let (pa, pb, pc, pd) = (pos(a), pos(b), pos(c2), pos(d2));
                        let w = weights(pa, pb, pc, pd);
                        ab_worst = (0..2)
                            .map(|k| (w[k] - stored[k]).abs())
                            .fold(ab_worst, f64::max);
                        let cd = sub(pd, pc);
                        let foot = |p: V| dot(sub(p, pc), cd) / dot(cd, cd);
                        if !(0.0..=1.0).contains(&foot(pa)) || !(0.0..=1.0).contains(&foot(pb)) {
                            beyond += 1;
                        } else {
                            let na = cross(cd, sub(pa, pc));
                            let nb = cross(sub(pb, pc), cd);
                            let fold = (dot(na, nb) / (len(na) * len(nb)).max(1e-30))
                                .clamp(-1.0, 1.0)
                                .acos()
                                .to_degrees();
                            let bucket = [1.0, 10.0, 45.0].iter().filter(|&&x| fold >= x).count();
                            by_fold[bucket]
                                .push((2..4).map(|k| (w[k] - stored[k]).abs()).fold(0.0, f64::max));
                        }
                        let sum_inv = inv(a) + inv(b) + inv(c2) + inv(d2);
                        part_betas.push(-f64::from(l.bend_stiffness) * sum_inv);
                        let (rc, current) = rest_curvature([pa, pb, pc, pd], stored);
                        let rest = f64::from(l.rest_curvature);
                        if (rest * current).abs() > 1e-4 {
                            curvature_rel.push((rc - rest).abs() / rest.abs());
                        }
                        most_height = most_height.max((rest * current).abs());
                    }
                    if let (Some(lo), Some(hi)) = (
                        part_betas.iter().copied().reduce(f64::min),
                        part_betas.iter().copied().reduce(f64::max),
                    ) {
                        if hi - lo > 1e-3 {
                            beta_mixed += 1;
                        } else {
                            *betas.entry(format!("{lo:.2}")).or_default() += 1;
                        }
                    }
                    if most_height > 0.0 {
                        height_ratio.push(
                            f64::from(*max_rest_pose_height_sq) / (most_height * most_height),
                        );
                    }
                    pairs_all += tri_pairs.len();
                    pairs_linked += tri_pairs.intersection(&linked).count();
                    links_elsewhere += linked.difference(&tri_pairs).count();
                }
            }
        }
        if has {
            with_set += 1;
        }
    }
    println!(
        "{} files, {with_set} with bend stiffness; {links_n} links",
        files.len()
    );
    println!("  weights of A and B: worst difference {ab_worst:.1e}");
    println!(
        "  weights of C and D, by how far the triangles are folded ({beyond} pairs with a far corner beyond the edge's ends left out):"
    );
    for (k, name) in ["under 1 deg", "1 to 10", "10 to 45", "45 and more"]
        .iter()
        .enumerate()
    {
        let v = &mut by_fold[k];
        let n = v.len();
        println!(
            "    {name}: {n} links, differences median {:.1e}, 99% {:.1e}",
            quantile(v, 0.5),
            quantile(v, 0.99)
        );
    }
    println!(
        "  rest curvature (links bent at rest): off the stored value by median {:.1e}, 90% {:.1e} of it",
        quantile(&mut curvature_rel, 0.5),
        quantile(&mut curvature_rel, 0.9)
    );
    println!(
        "  beta (-stiffness x the inverse masses summed), parts by it: {betas:?}; {beta_mixed} parts with several"
    );
    println!("  sets: {flags:?}");
    println!(
        "  maxRestPoseHeightSq / the set's largest height at rest, squared: median {:.2}, 10% {:.2}, 90% {:.2}",
        quantile(&mut height_ratio, 0.5),
        quantile(&mut height_ratio, 0.1),
        quantile(&mut height_ratio, 0.9)
    );
    println!(
        "  triangle pairs sharing an edge: {pairs_all}, with a link {pairs_linked}; links on no such pair {links_elsewhere}"
    );
}
