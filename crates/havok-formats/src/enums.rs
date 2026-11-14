//! Names for the values of the enumerated members the games' Havok files store as numbers.
//!
//! Each enumeration is a Rust enum with the value the files store ([`value`](hkp::MotionType::value)), the SDK's
//! name for it ([`name`](hkp::MotionType::name)) and the way back ([`from_value`](hkp::MotionType::from_value),
//! `None` for a value the enumeration does not name). Where the files hold only some of the named values the
//! variant says which, with the counts of a census of every distinct tagfile of Sekiro (19,127 read; 10 more name a
//! compendium that lies in another binder, and one archive entry is a truncated envelope) and Elden Ring (68,722)
//! and every one of the 490 packfiles of Tony Hawk's SHRED. A member that a type table marks as not serialisable
//! (declaration flag [`DeclFlags::NOT_SERIALIZABLE`](crate::DeclFlags::NOT_SERIALIZABLE)) stores no value: its bytes
//! read zero every time they were read, so the enumeration's zero there is no information.
//!
//! The names come from the sources that allow it. The 7.1 `hkp` enumerations of the SHRED packfiles that carry
//! their own class metadata ([`MotionType`](hkp::MotionType), [`ResponseType`](hkp::ResponseType),
//! [`BroadPhaseType`](hkp::BroadPhaseType)) are the metadata's own: the eight files list every item, and
//! `tests/enums.rs` compares them with these variants. The others of `hkp` are the 6.6 SDK headers' (the values
//! the 7.1 files store for them, 3 for the index striding type for example, are the headers' numbers). The 2014 and
//! later tagfiles name the enumerations of their members but list no items (Sekiro: 160 of 160 type tables that
//! describe `hkaAnimation`; Elden Ring: 660 of 660), so the `hka`, `hknp` and `hcl` names are those of HKLib's
//! 2018.1 classes (MIT), each one tested against the class that stores the value (`hknpShape`'s type against its
//! class, for example).
//!
//! A value that no file stores is still named when its source names it; the variant's documentation says it occurs
//! in no file.

macro_rules! named_enum {
    (
        $(#[$meta:meta])*
        enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $value:literal, $stored:literal; )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
        }

        impl $name {
            /// Every named value, in the order the SDK lists them.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The number the files store for the value.
            pub const fn value(self) -> i32 {
                match self {
                    $( $name::$variant => $value, )+
                }
            }

            /// The SDK's name of the value.
            pub const fn name(self) -> &'static str {
                match self {
                    $( $name::$variant => $stored, )+
                }
            }

            /// The named value a stored number stands for; `None` for a number the enumeration does not name.
            pub const fn from_value(value: i64) -> Option<$name> {
                match value {
                    $( $value => Some($name::$variant), )+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

/// Havok 7.1 physics (`hkp`), as the packfiles of Tony Hawk's SHRED store them (4-byte pointers, big endian). The
/// counts are over its 490 packfiles: 12,789 rigid bodies, 1,758 extended meshes with 6,445 subparts, 1,705 MOPP codes.
///
/// `hkpShape::type` has no enumeration here: the metadata of the SHRED files marks it `SERIALIZE_IGNORED`, and the
/// word is 0 in all 15,935 shapes (the class says what a shape is).
pub mod hkp {
    named_enum! {
        /// `hkpMotion::MotionType`, the byte at `+0xE8` of a rigid body (the motion's `type`). The items are those
        /// the eight reflection-bearing SHRED files list for the enumeration (`MOTION_MAX_ID = 8`, a count, is not
        /// a value). Rigid bodies store 2, 3 and 5 only.
        enum MotionType {
            /// 0, `MOTION_INVALID`: occurs in no body.
            Invalid = 0, "MOTION_INVALID";
            /// 1, `MOTION_DYNAMIC`: occurs in no body.
            Dynamic = 1, "MOTION_DYNAMIC";
            /// 2, `MOTION_SPHERE_INERTIA`: 396 bodies.
            SphereInertia = 2, "MOTION_SPHERE_INERTIA";
            /// 3, `MOTION_BOX_INERTIA`: 10,372 bodies, every one with a positive inverse mass and inertia.
            BoxInertia = 3, "MOTION_BOX_INERTIA";
            /// 4, `MOTION_KEYFRAMED`: occurs in no body.
            Keyframed = 4, "MOTION_KEYFRAMED";
            /// 5, `MOTION_FIXED`: 2,021 bodies, every one with zero inverse mass and inertia.
            Fixed = 5, "MOTION_FIXED";
            /// 6, `MOTION_THIN_BOX_INERTIA`: occurs in no body.
            ThinBoxInertia = 6, "MOTION_THIN_BOX_INERTIA";
            /// 7, `MOTION_CHARACTER`: occurs in no body.
            Character = 7, "MOTION_CHARACTER";
        }
    }

    named_enum! {
        /// `hkpMaterial::ResponseType`, the byte at `+0x8C` of a rigid body. The items are those the SHRED files list
        /// (`RESPONSE_MAX_ID = 4`, a count, is not a value). Every one of the 12,789 bodies stores 1.
        enum ResponseType {
            /// 0, `RESPONSE_INVALID`: occurs in no body.
            Invalid = 0, "RESPONSE_INVALID";
            /// 1, `RESPONSE_SIMPLE_CONTACT`: all 12,789 bodies.
            SimpleContact = 1, "RESPONSE_SIMPLE_CONTACT";
            /// 2, `RESPONSE_REPORTING`: occurs in no body.
            Reporting = 2, "RESPONSE_REPORTING";
            /// 3, `RESPONSE_NONE`: occurs in no body.
            None = 3, "RESPONSE_NONE";
        }
    }

    named_enum! {
        /// `hkpWorldObject::BroadPhaseType`, the byte at `+0x28` of a rigid body (the `type` of the collidable's
        /// broad-phase handle). The items are those the SHRED files list (`BROAD_PHASE_MAX_ID = 4`, a count, is not
        /// a value). Every one of the 12,789 bodies stores 1.
        enum BroadPhaseType {
            /// 0, `BROAD_PHASE_INVALID`: occurs in no body.
            Invalid = 0, "BROAD_PHASE_INVALID";
            /// 1, `BROAD_PHASE_ENTITY`: all 12,789 bodies.
            Entity = 1, "BROAD_PHASE_ENTITY";
            /// 2, `BROAD_PHASE_PHANTOM`: occurs in no body.
            Phantom = 2, "BROAD_PHASE_PHANTOM";
            /// 3, `BROAD_PHASE_BORDER`: occurs in no body.
            Border = 3, "BROAD_PHASE_BORDER";
        }
    }

    named_enum! {
        /// `hkpWeldingUtility::WeldingType`, the byte at `+0xD8` of an extended mesh (the 6.6 SDK's numbering; the
        /// SHRED files carry no metadata for it). All 1,758 meshes store 0, with a welding word per triangle.
        enum WeldingType {
            /// 0, `WELDING_TYPE_ANTICLOCKWISE`: all 1,758 meshes.
            Anticlockwise = 0, "WELDING_TYPE_ANTICLOCKWISE";
            /// 4, `WELDING_TYPE_CLOCKWISE`: occurs in no mesh.
            Clockwise = 4, "WELDING_TYPE_CLOCKWISE";
            /// 5, `WELDING_TYPE_TWO_SIDED`: occurs in no mesh.
            TwoSided = 5, "WELDING_TYPE_TWO_SIDED";
            /// 6, `WELDING_TYPE_NONE`: occurs in no mesh.
            None = 6, "WELDING_TYPE_NONE";
        }
    }

    named_enum! {
        /// `hkpExtendedMeshShape::IndexStridingType`, the byte at `+0x48` of a triangles subpart (the 6.6 SDK's
        /// numbering). All 6,445 subparts store 3: 32-bit indices, four to a triangle.
        enum IndexStridingType {
            /// 0, `INDICES_INVALID`: occurs in no subpart.
            Invalid = 0, "INDICES_INVALID";
            /// 1, `INDICES_INT8`: occurs in no subpart.
            Int8 = 1, "INDICES_INT8";
            /// 2, `INDICES_INT16`: occurs in no subpart.
            Int16 = 2, "INDICES_INT16";
            /// 3, `INDICES_INT32`: all 6,445 subparts.
            Int32 = 3, "INDICES_INT32";
        }
    }

    named_enum! {
        /// `hkpExtendedMeshShape::MaterialIndexStridingType`, the byte at `+0x01` of a triangles subpart (the 6.6
        /// SDK's numbering). All 6,445 subparts store 2: one 16-bit material index a triangle.
        enum MaterialIndexStridingType {
            /// 0, `MATERIAL_INDICES_INVALID`: occurs in no subpart.
            Invalid = 0, "MATERIAL_INDICES_INVALID";
            /// 1, `MATERIAL_INDICES_INT8`: occurs in no subpart.
            Int8 = 1, "MATERIAL_INDICES_INT8";
            /// 2, `MATERIAL_INDICES_INT16`: all 6,445 subparts.
            Int16 = 2, "MATERIAL_INDICES_INT16";
        }
    }

    named_enum! {
        /// `hkpExtendedMeshShape::SubpartType`, the byte at `+0x00` of a subpart (the 6.6 SDK's numbering). All 6,445
        /// subparts store 0.
        enum SubpartType {
            /// 0, `SUBPART_TRIANGLES`: all 6,445 subparts.
            Triangles = 0, "SUBPART_TRIANGLES";
            /// 1, `SUBPART_SHAPE`: occurs in no subpart.
            Shape = 1, "SUBPART_SHAPE";
        }
    }

    named_enum! {
        /// `hkpMoppCode::BuildType`, the byte at `+0x2C` of a MOPP code (the 6.6 SDK's numbering). All 1,705 codes
        /// store 0.
        enum MoppBuildType {
            /// 0, `BUILT_WITH_CHUNK_SUBDIVISION`: all 1,705 codes.
            BuiltWithChunkSubdivision = 0, "BUILT_WITH_CHUNK_SUBDIVISION";
            /// 1, `BUILT_WITHOUT_CHUNK_SUBDIVISION`: occurs in no code.
            BuiltWithoutChunkSubdivision = 1, "BUILT_WITHOUT_CHUNK_SUBDIVISION";
        }
    }
}

/// Havok animation (`hka`), as the 2015.1, 2016.2 and 2018.1 tagfiles store it. The counts are over every distinct
/// tagfile of Sekiro and Elden Ring.
pub mod hka {
    named_enum! {
        /// `hkaAnimationBinding::BlendHint`, a 32-bit value. The value 0 is `NORMAL` in every SDK; the names of 1 and 2
        /// differ by SDK, and the files do not say which composition they mean (the type tables name the enumeration
        /// and list no items): see [`BlendHint::name_in`]. Sekiro stores 0 in 14,121 bindings, 1 in 321 and 2 in 3 (all
        /// three of the character `c9520`); Elden Ring stores 0 in 44,645 and 1 in 4,136, and never 2.
        enum BlendHint {
            /// 0, `NORMAL`: an absolute pose.
            Normal = 0, "NORMAL";
            /// 1: `ADDITIVE_PARENT_SPACE` in the 2018.1 SDK (HKLib), `ADDITIVE_DEPRECATED` in the 2014 SDK.
            Additive1 = 1, "ADDITIVE_PARENT_SPACE";
            /// 2: `ADDITIVE_CHILD_SPACE` in the 2018.1 SDK (HKLib), `ADDITIVE` in the 2014 SDK.
            Additive2 = 2, "ADDITIVE_CHILD_SPACE";
        }
    }

    impl BlendHint {
        /// The name the SDK that wrote a tagfile gives the value, `sdk` being the tagfile's version string
        /// ([`Tagfile::version`](crate::Tagfile::version)): `NORMAL` for 0 in all of them, the 2018.1 names for
        /// `20180100`, and `None` where no source we may use names the value: 1 and 2 in `20160200` (Sekiro's SDK, which
        /// lies between the 2014 SDK, whose items were `ADDITIVE_DEPRECATED` and `ADDITIVE`, and 2018.1, whose items
        /// are the space names) and every value of `20150100` but 0.
        pub fn name_in(self, sdk: [u8; 8]) -> Option<&'static str> {
            match (self, &sdk) {
                (Self::Normal, _) => Some("NORMAL"),
                (Self::Additive1, b"20180100") => Some("ADDITIVE_PARENT_SPACE"),
                (Self::Additive2, b"20180100") => Some("ADDITIVE_CHILD_SPACE"),
                _ => None,
            }
        }
    }

    named_enum! {
        /// `hkaAnimatedReferenceFrame::hkaReferenceFrameTypeEnum`, the `frameType` member of a reference frame. The
        /// member is not serialised (declaration flags `0x21` in the type tables, bytes zero in all 27,474 reference
        /// frames: 6,865 of Sekiro and 20,609 of Elden Ring), so no file stores a value; the names are HKLib's.
        enum ReferenceFrameType {
            /// 0, `REFERENCE_FRAME_UNKNOWN`: the zero every reference frame's unstored member reads as.
            Unknown = 0, "REFERENCE_FRAME_UNKNOWN";
            /// 1, `REFERENCE_FRAME_DEFAULT`: stored by no file.
            Default = 1, "REFERENCE_FRAME_DEFAULT";
            /// 2, `REFERENCE_FRAME_PARAMETRIC`: stored by no file.
            Parametric = 2, "REFERENCE_FRAME_PARAMETRIC";
        }
    }
}

/// Havok NP physics (`hknp`), as the 2016.2 and 2018.1 tagfiles store it.
pub mod hknp {
    named_enum! {
        /// `hknpMotionType::Enum`, the `motionType` byte of an `hknpBodyCinfo`. The names are HKLib's (`NUM_TYPES = 3`,
        /// a count, is not a value). The values are the ones the census found by the mass of the body before the names
        /// were known: only dynamic bodies (2) have a positive mass, a keyframed body (1) has mass 0 or -1, and all
        /// map bodies are static. Over the `bodyCinfos` of every distinct tagfile: Sekiro 24,511 bodies (16,644 static,
        /// 642 keyframed, 7,225 dynamic), Elden Ring 313,074 (286,180, 404 and 26,490).
        enum MotionType {
            /// 0, `STATIC`: every map body; the mass is 0 or -1.
            Static = 0, "STATIC";
            /// 1, `KEYFRAMED`: the mass is 0 or -1, never positive.
            Keyframed = 1, "KEYFRAMED";
            /// 2, `DYNAMIC`: the only bodies with a positive mass (5,334 of Sekiro's 7,225, 24,257 of Elden Ring's 26,490).
            Dynamic = 2, "DYNAMIC";
        }
    }

    named_enum! {
        /// `hknpShapeType::Enum`, the `type` of an `hknpShape` (a 16-bit enum). The names are HKLib's (the `USER_`
        /// items follow `DUMMY`; `NUM_SHAPE_TYPES = 30` and `INVALID = 31` are a count and a sentinel). A class stores
        /// one value: `hknpConvexShape` 0, `hknpConvexPolytopeShape` 1, `hknpSphereShape` 2, `hknpCapsuleShape` 3,
        /// `hknpCylinderShape` 4, `hknpTriangleShape` 5, `hknpBoxShape` 6, `hknpCompressedMeshShape` and the game's
        /// `fsnpCustomParamCompressedMeshShape` 9, `hknpExternMeshShape` 10, in every shape of every distinct tagfile.
        enum ShapeType {
            /// 0, `CONVEX`.
            Convex = 0, "CONVEX";
            /// 1, `CONVEX_POLYTOPE`.
            ConvexPolytope = 1, "CONVEX_POLYTOPE";
            /// 2, `SPHERE`.
            Sphere = 2, "SPHERE";
            /// 3, `CAPSULE`.
            Capsule = 3, "CAPSULE";
            /// 4, `CYLINDER`.
            Cylinder = 4, "CYLINDER";
            /// 5, `TRIANGLE`.
            Triangle = 5, "TRIANGLE";
            /// 6, `BOX`.
            Box = 6, "BOX";
            /// 7, `DEBRIS`: stored by no file.
            Debris = 7, "DEBRIS";
            /// 8, `COMPOSITE`: stored by no file.
            Composite = 8, "COMPOSITE";
            /// 9, `COMPRESSED_MESH`.
            CompressedMesh = 9, "COMPRESSED_MESH";
            /// 10, `EXTERN_MESH`.
            ExternMesh = 10, "EXTERN_MESH";
            /// 11, `FX_PIPELINE`: stored by no file.
            FxPipeline = 11, "FX_PIPELINE";
            /// 12, `COMPOUND`: stored by no file.
            Compound = 12, "COMPOUND";
            /// 13, `DISTANCE_FIELD_BASE`: stored by no file.
            DistanceFieldBase = 13, "DISTANCE_FIELD_BASE";
            /// 14, `HEIGHT_FIELD`: stored by no file.
            HeightField = 14, "HEIGHT_FIELD";
            /// 15, `DISTANCE_FIELD`: stored by no file.
            DistanceField = 15, "DISTANCE_FIELD";
            /// 16, `PARTICLE_SYSTEM`: stored by no file.
            ParticleSystem = 16, "PARTICLE_SYSTEM";
            /// 17, `SCALED_CONVEX`: stored by no file.
            ScaledConvex = 17, "SCALED_CONVEX";
            /// 18, `MASKED`: stored by no file.
            Masked = 18, "MASKED";
            /// 19, `MASKED_COMPOUND`: stored by no file.
            MaskedCompound = 19, "MASKED_COMPOUND";
            /// 20, `LOD`: stored by no file.
            Lod = 20, "LOD";
            /// 21, `DUMMY`: stored by no file.
            Dummy = 21, "DUMMY";
            /// 22, `USER_0`: stored by no file.
            User0 = 22, "USER_0";
            /// 23, `USER_1`: stored by no file.
            User1 = 23, "USER_1";
            /// 24, `USER_2`: stored by no file.
            User2 = 24, "USER_2";
            /// 25, `USER_3`: stored by no file.
            User3 = 25, "USER_3";
            /// 26, `USER_4`: stored by no file.
            User4 = 26, "USER_4";
            /// 27, `USER_5`: stored by no file.
            User5 = 27, "USER_5";
            /// 28, `USER_6`: stored by no file.
            User6 = 28, "USER_6";
            /// 29, `USER_7`: stored by no file.
            User7 = 29, "USER_7";
        }
    }

    impl ShapeType {
        /// The value an `hknp` shape class stores in its `type`, by class name; `None` for a class no file of
        /// either game stores.
        pub fn of_class(class: &str) -> Option<Self> {
            Some(match class {
                "hknpConvexShape" => Self::Convex,
                "hknpConvexPolytopeShape" => Self::ConvexPolytope,
                "hknpSphereShape" => Self::Sphere,
                "hknpCapsuleShape" => Self::Capsule,
                "hknpCylinderShape" => Self::Cylinder,
                "hknpTriangleShape" => Self::Triangle,
                "hknpBoxShape" => Self::Box,
                "hknpCompressedMeshShape" | "fsnpCustomParamCompressedMeshShape" => Self::CompressedMesh,
                "hknpExternMeshShape" => Self::ExternMesh,
                _ => return None,
            })
        }
    }
}

/// Havok cloth (`hcl`), as the 2016.2 and 2018.1 tagfiles store it.
pub mod hcl {
    named_enum! {
        /// `hclShapeType`, the `type` of an `hclShape`. The member is not serialised (declaration flags `0x21`, bytes
        /// zero in every shape), so the class says what a shape is: see [`ShapeType::of_class`]. The names are HKLib's.
        enum ShapeType {
            /// 0, `HCL_SHAPE_TYPE_SPHERE` (`hclSphereShape`).
            Sphere = 0, "HCL_SHAPE_TYPE_SPHERE";
            /// 1, `HCL_SHAPE_TYPE_PLANE` (`hclPlaneShape`).
            Plane = 1, "HCL_SHAPE_TYPE_PLANE";
            /// 2, `HCL_SHAPE_TYPE_CAPSULE` (`hclCapsuleShape`).
            Capsule = 2, "HCL_SHAPE_TYPE_CAPSULE";
            /// 3, `HCL_SHAPE_TYPE_TAPERED_CAPSULE` (`hclTaperedCapsuleShape`).
            TaperedCapsule = 3, "HCL_SHAPE_TYPE_TAPERED_CAPSULE";
            /// 4, `HCL_SHAPE_TYPE_CYLINDER`: no class of either game's files.
            Cylinder = 4, "HCL_SHAPE_TYPE_CYLINDER";
            /// 5, `HCL_SHAPE_TYPE_CONVEX_HEIGHTFIELD` (`hclConvexHeightFieldShape`): in no file.
            ConvexHeightfield = 5, "HCL_SHAPE_TYPE_CONVEX_HEIGHTFIELD";
            /// 6, `HCL_SHAPE_TYPE_CONNECTED_MESH`: in no file.
            ConnectedMesh = 6, "HCL_SHAPE_TYPE_CONNECTED_MESH";
            /// 7, `HCL_SHAPE_TYPE_PLANAR_HEIGHTFIELD`: in no file.
            PlanarHeightfield = 7, "HCL_SHAPE_TYPE_PLANAR_HEIGHTFIELD";
            /// 8, `HCL_SHAPE_TYPE_DEFORMING_MESH`: in no file.
            DeformingMesh = 8, "HCL_SHAPE_TYPE_DEFORMING_MESH";
            /// 9, `HCL_SHAPE_TYPE_CONVEX_GEOMETRY` (`hclConvexGeometryShape`).
            ConvexGeometry = 9, "HCL_SHAPE_TYPE_CONVEX_GEOMETRY";
            /// 10, `HCL_SHAPE_TYPE_POINT_CONTACT_PLANES`: in no file.
            PointContactPlanes = 10, "HCL_SHAPE_TYPE_POINT_CONTACT_PLANES";
            /// 11, `HCL_SHAPE_TYPE_CONVEX_PLANES` (`hclConvexPlanesShape`): in no file.
            ConvexPlanes = 11, "HCL_SHAPE_TYPE_CONVEX_PLANES";
        }
    }

    impl ShapeType {
        /// The shape type of an `hcl` shape class, by class name; `None` for a class that is not one of the shape
        /// classes of HKLib's 2018.1 reflection. The classes the files hold are `hclCapsuleShape`,
        /// `hclTaperedCapsuleShape`, `hclSphereShape` (Sekiro and Elden Ring), `hclConvexGeometryShape` and
        /// `hclPlaneShape` (Elden Ring).
        pub fn of_class(class: &str) -> Option<Self> {
            Some(match class {
                "hclSphereShape" => Self::Sphere,
                "hclPlaneShape" => Self::Plane,
                "hclCapsuleShape" => Self::Capsule,
                "hclTaperedCapsuleShape" => Self::TaperedCapsule,
                "hclConvexHeightFieldShape" => Self::ConvexHeightfield,
                "hclConvexGeometryShape" => Self::ConvexGeometry,
                "hclConvexPlanesShape" => Self::ConvexPlanes,
                _ => return None,
            })
        }
    }

    named_enum! {
        /// `hclClothData::Platform`, the `targetPlatform` of a cloth (a 32-bit enum, one bit a platform). The names are
        /// HKLib's. Every `hclClothData` of both games stores 2: 610 in Sekiro's distinct tagfiles, 2,692 in Elden
        /// Ring's.
        enum Platform {
            /// 0, `HCL_PLATFORM_INVALID`: in no file.
            Invalid = 0, "HCL_PLATFORM_INVALID";
            /// 1, `HCL_PLATFORM_WIN32`: in no file.
            Win32 = 1, "HCL_PLATFORM_WIN32";
            /// 2, `HCL_PLATFORM_X64`: all 3,302 cloths.
            X64 = 2, "HCL_PLATFORM_X64";
            /// 4, `HCL_PLATFORM_MACPPC`: in no file.
            MacPpc = 4, "HCL_PLATFORM_MACPPC";
            /// 8, `HCL_PLATFORM_IOS`: in no file.
            Ios = 8, "HCL_PLATFORM_IOS";
            /// 16, `HCL_PLATFORM_MAC386`: in no file.
            Mac386 = 16, "HCL_PLATFORM_MAC386";
            /// 32, `HCL_PLATFORM_PS3`: in no file.
            Ps3 = 32, "HCL_PLATFORM_PS3";
            /// 64, `HCL_PLATFORM_XBOX360`: in no file.
            Xbox360 = 64, "HCL_PLATFORM_XBOX360";
            /// 128, `HCL_PLATFORM_WII`: in no file.
            Wii = 128, "HCL_PLATFORM_WII";
            /// 256, `HCL_PLATFORM_LRB`: in no file.
            Lrb = 256, "HCL_PLATFORM_LRB";
            /// 512, `HCL_PLATFORM_LINUX`: in no file.
            Linux = 512, "HCL_PLATFORM_LINUX";
            /// 1024, `HCL_PLATFORM_PSVITA`: in no file.
            PsVita = 1024, "HCL_PLATFORM_PSVITA";
            /// 2048, `HCL_PLATFORM_ANDROID`: in no file.
            Android = 2048, "HCL_PLATFORM_ANDROID";
            /// 4096, `HCL_PLATFORM_CTR`: in no file.
            Ctr = 4096, "HCL_PLATFORM_CTR";
            /// 8192, `HCL_PLATFORM_WIIU`: in no file.
            WiiU = 8192, "HCL_PLATFORM_WIIU";
            /// 16384, `HCL_PLATFORM_PS4`: in no file.
            Ps4 = 16384, "HCL_PLATFORM_PS4";
            /// 32768, `HCL_PLATFORM_XBOXONE`: in no file.
            XboxOne = 32768, "HCL_PLATFORM_XBOXONE";
            /// 65536, `HCL_PLATFORM_MAC64`: in no file.
            Mac64 = 65536, "HCL_PLATFORM_MAC64";
            /// 131072, `HCL_PLATFORM_NX`: in no file.
            Nx = 131072, "HCL_PLATFORM_NX";
            /// 262144, `HCL_PLATFORM_GDK`: in no file.
            Gdk = 262144, "HCL_PLATFORM_GDK";
            /// 524288, `HCL_PLATFORM_COOKER`: in no file.
            Cooker = 524288, "HCL_PLATFORM_COOKER";
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{hcl, hka, hknp, hkp};

    /// Every enumeration: each value comes back as itself by number and prints as its name; values and names do not
    /// repeat; a number that is none of them is `None`.
    macro_rules! round_trip {
        ($($t:ty),+ $(,)?) => {$({
            let all = <$t>::ALL;
            assert!(!all.is_empty(), "{}", stringify!($t));
            for &v in all {
                assert_eq!(<$t>::from_value(i64::from(v.value())), Some(v), "{v}");
                assert_eq!(v.to_string(), v.name());
            }
            let mut values: Vec<i32> = all.iter().map(|v| v.value()).collect();
            values.sort_unstable();
            values.dedup();
            assert_eq!(values.len(), all.len(), "{}: a value repeats", stringify!($t));
            let mut names: Vec<&str> = all.iter().map(|v| v.name()).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), all.len(), "{}: a name repeats", stringify!($t));
            assert_eq!(<$t>::from_value(-1), None);
            assert_eq!(<$t>::from_value(1 << 40), None);
        })+};
    }

    #[test]
    fn every_enumeration_round_trips() {
        round_trip!(
            hkp::MotionType,
            hkp::ResponseType,
            hkp::BroadPhaseType,
            hkp::WeldingType,
            hkp::IndexStridingType,
            hkp::MaterialIndexStridingType,
            hkp::SubpartType,
            hkp::MoppBuildType,
            hka::BlendHint,
            hka::ReferenceFrameType,
            hknp::MotionType,
            hknp::ShapeType,
            hcl::ShapeType,
            hcl::Platform,
        );
    }

    /// The numbers the SDKs give the values that matter to a reader of the games' files.
    #[test]
    fn the_values_are_the_sdks_numbers() {
        assert_eq!(hkp::MotionType::SphereInertia.value(), 2);
        assert_eq!(hkp::MotionType::BoxInertia.value(), 3);
        assert_eq!(hkp::MotionType::Fixed.value(), 5);
        assert_eq!(hkp::ResponseType::SimpleContact.value(), 1);
        assert_eq!(hkp::BroadPhaseType::Entity.value(), 1);
        assert_eq!(hkp::WeldingType::Anticlockwise.value(), 0);
        assert_eq!(hkp::WeldingType::None.value(), 6);
        assert_eq!(hkp::IndexStridingType::Int32.value(), 3);
        assert_eq!(hkp::MaterialIndexStridingType::Int16.value(), 2);
        assert_eq!(hkp::SubpartType::Triangles.value(), 0);
        assert_eq!(hkp::MoppBuildType::BuiltWithChunkSubdivision.value(), 0);
        assert_eq!(hknp::MotionType::from_value(1), Some(hknp::MotionType::Keyframed));
        assert_eq!(hknp::MotionType::from_value(2), Some(hknp::MotionType::Dynamic));
        assert_eq!(hknp::MotionType::from_value(3), None);
        assert_eq!(hknp::ShapeType::CompressedMesh.value(), 9);
        assert_eq!(hknp::ShapeType::ExternMesh.value(), 10);
        assert_eq!(hcl::Platform::X64.value(), 2);
        assert_eq!(hcl::Platform::Cooker.value(), 524_288);
        assert_eq!(hcl::ShapeType::ConvexGeometry.value(), 9);
    }

    #[test]
    fn a_shape_class_names_its_type() {
        for (class, want) in [
            ("hknpConvexShape", hknp::ShapeType::Convex),
            ("hknpBoxShape", hknp::ShapeType::Box),
            ("hknpCompressedMeshShape", hknp::ShapeType::CompressedMesh),
            (
                "fsnpCustomParamCompressedMeshShape",
                hknp::ShapeType::CompressedMesh,
            ),
            ("hknpExternMeshShape", hknp::ShapeType::ExternMesh),
        ] {
            assert_eq!(hknp::ShapeType::of_class(class), Some(want), "{class}");
        }
        assert_eq!(hknp::ShapeType::of_class("hknpShape"), None);
        assert_eq!(
            hcl::ShapeType::of_class("hclCapsuleShape"),
            Some(hcl::ShapeType::Capsule)
        );
        assert_eq!(
            hcl::ShapeType::of_class("hclTaperedCapsuleShape"),
            Some(hcl::ShapeType::TaperedCapsule)
        );
        assert_eq!(hcl::ShapeType::of_class("hclShape"), None);
    }

    #[test]
    fn a_blend_hint_is_named_by_the_sdk_that_wrote_it() {
        use hka::BlendHint::{Additive1, Additive2, Normal};
        for sdk in [*b"20150100", *b"20160200", *b"20180100"] {
            assert_eq!(Normal.name_in(sdk), Some("NORMAL"));
        }
        assert_eq!(Additive1.name_in(*b"20180100"), Some("ADDITIVE_PARENT_SPACE"));
        assert_eq!(Additive2.name_in(*b"20180100"), Some("ADDITIVE_CHILD_SPACE"));
        // no source we may use names them for the SDKs between and before
        assert_eq!(Additive1.name_in(*b"20160200"), None);
        assert_eq!(Additive2.name_in(*b"20150100"), None);
    }
}
