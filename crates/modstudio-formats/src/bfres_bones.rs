//! Bones added to a model's skeleton ([`with_bones`]), as [`crate::bfres_write`] writes them: after the game's bones,
//! each with a smooth matrix after the game's smooth ones (so the rigid matrices, after those, move up by the
//! number added) and an inverse bind matrix from its place at rest.
//!
//! A bone's flags (worked out from the game's 51,936 bones, `examples/bone_flags_check.rs`): bit 0 visible, bits 12
//! to 14 the rotation mode (`0x1000`: Euler angles XYZ), bits 16 to 18 billboarding, bit 23 found on most bones;
//! bits 24 to 27 say of its own transform that the scale is uniform, the scale's volume is one, the rotation is none,
//! the translation is none (exactly so on every game bone); bits 28 to 31, the same down from the root, are 0 on the
//! game's bones. A new bone takes visible, the rotation mode and bit 23 from its parent, and its own transform bits.

use std::collections::HashSet;

use crate::bfres::{Bone, Skeleton};
use crate::error::{Result, invalid};

/// A bone added to a skeleton.
#[derive(Debug, Clone, PartialEq)]
pub struct NewBone {
    pub name: String,
    /// Its parent: a bone of the skeleton, or a new bone before it (counted after the skeleton's).
    pub parent: usize,
    pub scale: [f32; 3],
    /// As the skeleton keeps rotations: Euler angles XYZ (radians; the fourth is 1, as the game's are), or a
    /// quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
}

/// The flag bits a new bone takes from its parent: visible, the rotation mode, bit 23.
const FROM_PARENT: u32 = 0x1 | 0x7000 | 0x80_0000;

/// The flag bits saying what a bone's own transform is (24 to 27).
pub fn transform_flags(
    scale: [f32; 3],
    rotation: [f32; 4],
    translation: [f32; 3],
    euler: bool,
) -> u32 {
    let uniform = scale[0] == scale[1] && scale[1] == scale[2];
    let volume_one = scale[0] * scale[1] * scale[2] == 1.0;
    let no_rotation = rotation[0] == 0.0
        && rotation[1] == 0.0
        && rotation[2] == 0.0
        && (euler || rotation[3] == 1.0);
    let no_translation = translation == [0.0; 3];
    (uniform as u32) << 24
        | (volume_one as u32) << 25
        | (no_rotation as u32) << 26
        | (no_translation as u32) << 27
}

/// A row-major 4x4.
pub type Matrix = [[f32; 4]; 4];

fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut m = [[0f32; 4]; 4];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    m
}

/// A bone's place relative to its parent: translation, then rotation (Euler XYZ: Z after Y after X; or a
/// quaternion), then scale.
pub fn local_matrix(
    scale: [f32; 3],
    rotation: [f32; 4],
    translation: [f32; 3],
    euler: bool,
) -> Matrix {
    let r: [[f32; 3]; 3] = if euler {
        let (sx, cx) = rotation[0].sin_cos();
        let (sy, cy) = rotation[1].sin_cos();
        let (sz, cz) = rotation[2].sin_cos();
        [
            [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
            [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
            [-sy, sx * cy, cx * cy],
        ]
    } else {
        let [x, y, z, w] = rotation;
        [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - w * z),
                2.0 * (x * z + w * y),
            ],
            [
                2.0 * (x * y + w * z),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - w * x),
            ],
            [
                2.0 * (x * z - w * y),
                2.0 * (y * z + w * x),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ]
    };
    let mut m = [[0f32; 4]; 4];
    for row in 0..3 {
        for col in 0..3 {
            m[row][col] = r[row][col] * scale[col];
        }
        m[row][3] = translation[row];
    }
    m[3][3] = 1.0;
    m
}

/// Each bone's place in the model at rest, the skeleton's order (a bone whose parent comes after it is placed once
/// its parent is).
pub fn rest_world(bones: &[Bone], euler: bool) -> Vec<Matrix> {
    let mut world: Vec<Option<Matrix>> = vec![None; bones.len()];
    fn place(
        bones: &[Bone],
        i: usize,
        euler: bool,
        world: &mut [Option<Matrix>],
        depth: usize,
    ) -> Matrix {
        if let Some(m) = world[i] {
            return m;
        }
        let b = &bones[i];
        let local = local_matrix(b.scale, b.rotation, b.translation, euler);
        let m = match b.parent {
            Some(p) if p != i && p < bones.len() && depth < 1024 => {
                mul(&place(bones, p, euler, world, depth + 1), &local)
            }
            _ => local,
        };
        world[i] = Some(m);
        m
    }
    (0..bones.len())
        .map(|i| place(bones, i, euler, &mut world, 0))
        .collect()
}

/// The inverse of an affine matrix, as the 3 rows of 4 the file keeps (`None`: it has no inverse).
pub fn inverse_rows(m: &Matrix) -> Option<[f32; 12]> {
    let a = |r: usize, c: usize| m[r][c] as f64;
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    if det.abs() < 1e-20 {
        return None;
    }
    let inv = [
        [
            (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) / det,
            (a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2)) / det,
            (a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1)) / det,
        ],
        [
            (a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2)) / det,
            (a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0)) / det,
            (a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2)) / det,
        ],
        [
            (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0)) / det,
            (a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1)) / det,
            (a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0)) / det,
        ],
    ];
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    let mut out = [0f32; 12];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 4 + c] = inv[r][c] as f32;
        }
        out[r * 4 + 3] = -(0..3).map(|k| inv[r][k] * t[k]).sum::<f64>() as f32;
    }
    Some(out)
}

/// The skeleton with `new` bones after its own: each new bone with a smooth matrix after the skeleton's smooth ones
/// (the rigid matrices move up by the number added, and the bones' rigid indices with them), and an inverse bind
/// matrix from its place at rest. Its `at` is 0 (it isn't in the file yet).
pub fn with_bones(skeleton: &Skeleton, new: &[NewBone]) -> Result<Skeleton> {
    let n = skeleton.bones.len();
    let s = skeleton.smooth_count;
    let k = new.len();
    let euler = skeleton.euler_rotations();
    let mut names: HashSet<&str> = skeleton.bones.iter().map(|b| b.name.as_str()).collect();
    if n + k > u16::MAX as usize || s + k + skeleton.rigid_count > u16::MAX as usize {
        return Err(invalid(format!("{} bones in all: too many", n + k)));
    }
    let mut bones = skeleton.bones.clone();
    for b in &mut bones {
        if let Some(r) = b.rigid_matrix.filter(|&r| r >= s) {
            b.rigid_matrix = Some(r + k);
        }
    }
    for (j, nb) in new.iter().enumerate() {
        if nb.name.is_empty() || nb.name.len() > u16::MAX as usize {
            return Err(invalid(format!(
                "new bone {j}: a name of {} bytes",
                nb.name.len()
            )));
        }
        if !names.insert(nb.name.as_str()) {
            return Err(invalid(format!(
                "new bone {}: the skeleton has a bone of that name",
                nb.name
            )));
        }
        if nb.parent >= n + j {
            return Err(invalid(format!(
                "new bone {}: its parent {} isn't a bone before it",
                nb.name, nb.parent
            )));
        }
        let values = nb.scale.iter().chain(&nb.rotation).chain(&nb.translation);
        if values.clone().any(|v| !v.is_finite()) {
            return Err(invalid(format!(
                "new bone {}: its rest transform isn't finite",
                nb.name
            )));
        }
        let flags = (bones[nb.parent].flags & FROM_PARENT)
            | transform_flags(nb.scale, nb.rotation, nb.translation, euler);
        bones.push(Bone {
            at: 0,
            name: nb.name.clone(),
            parent: Some(nb.parent),
            smooth_matrix: Some(s + j),
            rigid_matrix: None,
            flags,
            scale: nb.scale,
            rotation: nb.rotation,
            translation: nb.translation,
        });
    }
    let mut matrix_to_bone: Vec<u16> = skeleton.matrix_to_bone[..s].to_vec();
    matrix_to_bone.extend((n..n + k).map(|b| b as u16));
    matrix_to_bone.extend_from_slice(&skeleton.matrix_to_bone[s..]);
    let world = rest_world(&bones, euler);
    let mut inverse_model_matrices = skeleton.inverse_model_matrices.clone();
    for (j, nb) in new.iter().enumerate() {
        inverse_model_matrices.push(inverse_rows(&world[n + j]).ok_or_else(|| {
            invalid(format!(
                "new bone {}: its place at rest can't be undone (scaled to 0)",
                nb.name
            ))
        })?);
    }
    Ok(Skeleton {
        at: skeleton.at,
        flags: skeleton.flags,
        bones,
        matrix_to_bone,
        smooth_count: s + k,
        rigid_count: skeleton.rigid_count,
        inverse_model_matrices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(name: &str, parent: Option<usize>, t: [f32; 3], r: [f32; 4]) -> Bone {
        Bone {
            at: 0,
            name: name.into(),
            parent,
            smooth_matrix: None,
            rigid_matrix: None,
            flags: 0x80_1001,
            scale: [1.0; 3],
            rotation: r,
            translation: t,
        }
    }

    /// Root, Arm (smooth, 1 along x, turned a quarter about z), Eye (rigid).
    fn skeleton() -> Skeleton {
        let mut b = vec![
            bone("Root", None, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
            bone(
                "Arm",
                Some(0),
                [1.0, 0.0, 0.0],
                [0.0, 0.0, std::f32::consts::FRAC_PI_2, 1.0],
            ),
            bone("Eye", Some(0), [0.0, 2.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
        ];
        b[0].smooth_matrix = Some(0);
        b[1].smooth_matrix = Some(1);
        b[2].rigid_matrix = Some(2);
        let world = rest_world(&b, true);
        Skeleton {
            at: 0,
            flags: 0x1000,
            bones: b,
            matrix_to_bone: vec![0, 1, 2],
            smooth_count: 2,
            rigid_count: 1,
            inverse_model_matrices: vec![
                inverse_rows(&world[0]).unwrap(),
                inverse_rows(&world[1]).unwrap(),
            ],
        }
    }

    #[test]
    fn new_bones_go_after_the_smooth_matrices_and_the_rigid_ones_move_up() {
        let s = skeleton();
        let hand = NewBone {
            name: "Hand".into(),
            parent: 1,
            scale: [1.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.5, 0.0, 0.0],
        };
        let tip = NewBone {
            name: "Tip".into(),
            parent: 3,
            translation: [0.25, 0.0, 0.0],
            ..hand.clone()
        };
        let x = with_bones(&s, &[hand.clone(), tip]).unwrap();
        assert_eq!(x.bones.len(), 5);
        assert_eq!(x.matrix_to_bone, [0, 1, 3, 4, 2]);
        assert_eq!((x.smooth_count, x.rigid_count), (4, 1));
        assert_eq!(x.bones[2].rigid_matrix, Some(4));
        assert_eq!(x.bones[3].smooth_matrix, Some(2));
        assert_eq!(x.bones[4].parent, Some(3));
        // visible, Euler, bit 23 from the parent; scale one, no rotation
        assert_eq!(x.bones[3].flags, 0x80_1001 | 0x0700_0000);
        // Hand is 0.5 along Arm's x, which the turn points along the model's y: at (1, 0.5, 0); its inverse bind
        // takes that point to its origin
        let inv = x.inverse_model_matrices[2];
        let at = |p: [f32; 3]| {
            [0, 1, 2].map(|r| {
                inv[r * 4] * p[0] + inv[r * 4 + 1] * p[1] + inv[r * 4 + 2] * p[2] + inv[r * 4 + 3]
            })
        };
        let o = at([1.0, 0.5, 0.0]);
        assert!(o.iter().all(|v| v.abs() < 1e-6), "{o:?}");
        let tip_origin = [0, 1, 2].map(|r| {
            let m = x.inverse_model_matrices[3];
            m[r * 4] + m[r * 4 + 1] * 0.75 + m[r * 4 + 3]
        });
        assert!(tip_origin.iter().all(|v| v.abs() < 1e-6), "{tip_origin:?}");
    }

    #[test]
    fn names_and_parents_are_checked() {
        let s = skeleton();
        let mut b = NewBone {
            name: "Arm".into(),
            parent: 0,
            scale: [1.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.0; 3],
        };
        assert!(with_bones(&s, std::slice::from_ref(&b)).is_err());
        b.name = "New".into();
        b.parent = 3;
        assert!(with_bones(&s, std::slice::from_ref(&b)).is_err());
        b.parent = 2;
        b.scale = [0.0; 3];
        assert!(with_bones(&s, &[b]).is_err());
    }

    #[test]
    fn transform_flags_as_the_game_sets_them() {
        assert_eq!(
            transform_flags([1.0; 3], [0.0, 0.0, 0.0, 1.0], [0.0; 3], true),
            0x0F00_0000
        );
        assert_eq!(
            transform_flags([2.0; 3], [0.1, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0], true),
            0x0100_0000
        );
        assert_eq!(
            transform_flags([0.5, 2.0, 1.0], [0.0, 0.0, 0.0, 0.0], [0.0; 3], false),
            0x0A00_0000
        );
    }
}
