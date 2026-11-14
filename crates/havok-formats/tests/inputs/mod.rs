//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether a run that
//! requires every gate input fails without it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Sekiro = "sekiro",
        Elden = "elden",
        Shred = "shred",
    }
    installs {}
    keys {}
    tools {}
    oracle_sets {
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (here: Havok tagfiles and their compendium)",
            source: "the user's own installs: files extracted as stored",
        },
    }
    declared {
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok") => {
            status: Live,
            gate: true,
            about: r#"one area's high-resolution map collision as its binder stores it: loose Havok 2016.2 tagfiles (`.hkx`) and the type compendium they share (`.compendium`), every body and compressed mesh read by tests/corpus.rs"#,
            source: r#"the game's own files from the user's install: the area's `h*.hkxbhd`/`.hkxbdt` binder with each member taken out of its DCX envelope, as [corpus_sekiro_havok.rs](https://github.com/Kalmarv/primase/tree/main/worlds/fromsoft/examples/corpus_sekiro_havok.rs) in primase does"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-2015") => {
            status: Live,
            gate: false,
            about: r#"the one SDK 2015.1 tagfile of the Sekiro install (`c1090`'s clip `a000_205000`: 54 transform tracks, 36 frames), loose; tests/sdk2015.rs reads it, tests/spline_encode.rs writes its blocks back, re-encodes it and writes it into its file"#,
            source: r#"the game's own file from the user's install: the member of `chr/c1090.anibnd.dcx` out of its DCX envelope, found by reading every Havok tagfile of the install for the SDK string `20150100`"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-float") => {
            status: Live,
            gate: false,
            about: r#"two Sekiro cutscene tagfiles with a float track (the camera's field of view): `s11_00_0020` cut0010 (two 16-bit spline blocks) and `s11_01_0031` cut0000 (a spline block, then a static one), loose, each with its own type table; tests/float_tracks.rs reads them, tests/spline_encode.rs writes their blocks back, re-encodes them and writes them into their files"#,
            source: r#"the game's own files from the user's install: the members `cut0010/a0000.hkx` of `cutscene/s11_00_0020.cutscenebnd.dcx` and `cut0000/a0000.hkx` of `cutscene/s11_01_0031.cutscenebnd.dcx`, as stored, found by reading every cutscene binder of the install for animations with float tracks"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-l") => {
            status: Live,
            gate: false,
            about: r#"the one Sekiro tagfile whose mesh trees have sections of page above 0 (89 sections; the low-resolution map collision `l10_00_00_00`, member `_000001`) and the compendium it shares, loose; tests/paged_mesh.rs decodes every mesh of it"#,
            source: r#"the game's own files from the user's install: the members of the `l10_00_00_00.hkxbhd`/`.hkxbdt` binder out of their DCX envelopes, picked by reading every map `l` binder of the install for sections of page above 0"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-cloth") => {
            status: Live,
            gate: false,
            about: r#"every distinct tagfile with cloth (`hclClothData` in its type table) of the install: 149 files, loose, and `manifest.tsv` (sha256, file, compendium, binder, member) naming each of the 167 binder members they were found in; tests/cloth_corpus.rs reads them all"#,
            source: r#"the game's own files from the user's install: every archive entry walked (named or not, nested BND4 and DCX opened), each TAG0 member whose type table has `hclClothData` taken out of its DCX envelope and deduplicated by sha256 (95 `chrbnd`, 36 `objbnd`, 36 `partsbnd` members)"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-cloth") => {
            status: Live,
            gate: false,
            about: r#"every distinct tagfile with cloth of the install: 814 files, loose, and `manifest.tsv` naming each of the 1,605 binder members they were found in; tests/cloth_corpus.rs reads them all"#,
            source: r#"the game's own files from the user's install, found as for Sekiro (182 `chrbnd`, 1,418 `partsbnd`, 5 `geombnd` members)"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-float") => {
            status: Live,
            gate: false,
            about: r#"three Elden Ring cutscene tagfiles, loose, each with its own type table: `s13_00_0040` cut0040 (a camera whose float track is a degree 1 spline) and the two animations of `s12_05_0020` (cut0040, cut0060) whose wing-bone rotation spline stores one control point of four NaNs; tests/float_tracks.rs reads them, tests/spline_encode.rs writes their blocks back, re-encodes them (not the two with NaNs) and writes them into their files"#,
            source: r#"the game's own files from the user's install: the members `cut0040/a0000.hkx` of `cutscene/s13_00_0040.cutscenebnd.dcx` and `cut0040/a0000.hkx` and `cut0060/a0000.hkx` of `cutscene/s12_05_0020.cutscenebnd.dcx`, as stored, found by reading every cutscene binder of the install for float tracks and for rotation floats that are not finite"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-l") => {
            status: Live,
            gate: false,
            about: r#"the one Elden Ring tagfile whose mesh trees have sections of page above 0 (6,233 sections, pages 1 to 4; the low-resolution map collision `l14_00_00_00`, member `_000000`) and the compendium it shares, loose; tests/paged_mesh.rs decodes every mesh of it"#,
            source: r#"the game's own files from the user's install: the members of the `l14_00_00_00.hkxbhd`/`.hkxbdt` binder out of their DCX envelopes, picked by reading every map `l` binder of the install for sections of page above 0"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-behavior") => {
            status: Live,
            gate: false,
            about: r#"every distinct behaviour tagfile of the install (an `hkb*` or `Custom*` class in its type table): 305 files, loose, and `manifest.tsv` (sha256, file, compendium, binder, member) naming each of the 315 binder members they were found in (147 behaviour graphs, 79 characters and 79 projects); tests/behavior.rs reads them all through the typed views"#,
            source: r#"the game's own files from the user's install: every archive entry walked (named or not, nested BND4 and DCX opened), each TAG0 member whose type table has an `hkb*` or `Custom*` class taken out of its DCX envelope and deduplicated by sha256 (all in the 79 `chr/*.behbnd.dcx` binders)"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-behavior") => {
            status: Live,
            gate: false,
            about: r#"every distinct behaviour tagfile of the install: 1,021 files, loose, and `manifest.tsv` naming each of the 1,119 binder members they were found in (461 behaviour graphs, 280 characters and 280 projects); tests/behavior.rs reads them all through the typed views"#,
            source: r#"the game's own files from the user's install, found as for Sekiro (all in the 280 `chr/*.behbnd.dcx` binders)"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-behavior-xml") => {
            status: Live,
            gate: false,
            about: r#"HKLib's XML (`hktagfile` version 3) of twelve Elden Ring behaviour tagfiles of the `havok-behavior` set (3 to 36,527 objects: 50,196 in all), loose, named `<tagfile>.xml`; tests/xml_oracle.rs compares them with this crate's objects of the same files"#,
            source: r#"written by HKLib (MIT, pinned at ac40bc9, built for .NET 8) from the files of `havok-behavior`: its `HavokBinarySerializer` reads each tagfile and its `HavokXmlSerializer` writes the XML; HKLib reads only 2018.1 tagfiles, so Sekiro has none"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Shred, "havok-packfiles") => {
            status: Live,
            gate: true,
            about: r#"the 490 Havok packfiles (version 7, `Havok-7.1.0-r1`, 4-byte pointers, big endian) of Tony Hawk's SHRED for the PS3: the level and ragdoll collision files (`.hkw`), loose; tests/packfile_image.rs reads, images and writes each of them back byte for byte"#,
            source: r#"the game's own files from the user's disc image: every entry of the disc's archives that starts with the packfile magic, as stored"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-packfiles") => {
            status: Live,
            gate: false,
            about: r#"the 29 distinct packfiles of the install (26 of version 11, `hk_2014.1.0-r1`, and 3 of version 9, `hk_2013.1.0-r1`; 8-byte pointers, little endian), loose and named by their sha256; tests/packfile_image.rs reads, images and writes each of them back byte for byte"#,
            source: r#"the game's own files from the user's install: every archive entry walked (named or not, nested BND4 and DCX opened), each blob that starts with the packfile magic taken out of its DCX envelope and deduplicated by sha256 (25 of the version 11 files in seven `partsbnd`, the other, `c9996.hkx`, a behaviour file in ten `behbnd` copies, and the three version 9 files)"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-packfiles") => {
            status: Live,
            gate: false,
            about: r#"the 6 distinct packfiles of the install (3 of version 11 and 3 of version 9, all face animation), loose and named by their sha256; tests/packfile_image.rs reads, images and writes each of them back byte for byte"#,
            source: r#"the game's own files from the user's install, found as for Sekiro"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-nav") => {
            status: Live,
            gate: false,
            about: r#"five navmesh tagfiles of the install (`hkaiNavMesh` with an `hkaiDirectedGraphExplicitCost`, Sekiro's 2016.2 files), loose, each carrying its own type table, and `manifest.tsv` with the counts of each (faces, edges, user edges, vertices, clustered faces, triangles, graph nodes and edges, user-edge setups, data words): the smallest and the largest with a graph, an empty one, the smallest and the largest with user-edge setups; tests/navmesh.rs reads them"#,
            source: r#"the game's own files from the user's install: members of the `m10`, `m17`, `m20` and `m25` `nvmhktbnd` binders, out of their DCX envelopes as stored, picked by reading all 1,509 navmesh tagfiles of the 10 binders"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-nav") => {
            status: Live,
            gate: false,
            about: r#"seven navmesh tagfiles of the install (`hkaiNavMesh` beside an `hkaiStaticTreeNavMeshQueryMediator` and, in two, an `hkaiUserEdgeSetupArray`, Elden Ring's 2018.1 files), loose, each carrying its own type table, and `manifest.tsv` with the counts of each: an empty mesh, the smallest and the largest with a tree, an empty tree, the smallest and the largest with user edges, and the one with a face that ear clipping cannot finish; tests/navmesh.rs reads them"#,
            source: r#"the game's own files from the user's install: members of the `nvmhktbnd` binders of `m10`, `m12`, `m13`, `m34`, `m60_47_38` and `m60_49_37`, out of their DCX envelopes as stored, picked by reading all 7,438 navmesh tagfiles of the 695 binders"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-enums") => {
            status: Live,
            gate: false,
            about: r#"the smallest self-contained or compendium-bound tagfile of the Sekiro install that shows each value of the enumerated members tests/enums.rs names: 24 tagfiles with 5 compendia, loose, and `manifest.tsv` (file, size, binder member, the values it was picked for); tests/enums.rs and tests/mesh.rs read them"#,
            source: r#"the game's own files from the user's install: every archive entry walked (named or not, nested BND4 and DCX opened), each distinct TAG0 member counted for the values of the enumerated members of the animation, collision and cloth classes, and for each value the smallest tagfile with it taken out of its DCX envelope, with the compendium its types need"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-enums") => {
            status: Live,
            gate: false,
            about: r#"the same for the Elden Ring install: 29 tagfiles with 3 compendia, loose, and `manifest.tsv`; tests/enums.rs reads them"#,
            source: r#"the game's own files from the user's install, found as for Sekiro"#,
        },
    }
    debug {}
}
