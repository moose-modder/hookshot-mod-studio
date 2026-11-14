//! Real-data check of the behaviour views: every behaviour tagfile of the Sekiro and Elden Ring installs (`chr/*.behbnd.dcx`:
//! behaviour graphs, characters and projects), extracted from the user's own install (see tests/inputs/README.md); skips
//! without them. No game data is included in the repository.
//!
//! The expected values (tests/behavior_expected/mod.rs) were counted without this crate, by a census that parsed the type
//! tables and walked the item tables of the same files from their bytes: the objects and structure elements of each class, the
//! enumeration values against HKLib's member names, the tables that tie a behaviour graph together, and sums over the
//! members of clip generators, states, transitions and manual selectors decoded by a second reader.
mod behavior_expected;
mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use behavior_expected::{ELDEN, Expected, SEKIRO};
use havok_formats::behavior::{Class, ClassCounts, Document, FileKind, Typed};
use havok_formats::{ItemKind, Tagfile};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

type Failure = Box<dyn std::error::Error>;

/// What one pass over a game's files counted.
#[derive(Default)]
struct Found {
    files: usize,
    behaviors: usize,
    characters: usize,
    projects: usize,
    counts: ClassCounts,
    typed_objects: usize,
    typed_elements: usize,
    untyped: BTreeMap<String, usize>,
    members_read: usize,
    enum_members: usize,
    unnamed_enum_members: usize,
    variables: usize,
    events: usize,
    graph_properties: usize,
    character_properties: usize,
    variable_bindings: usize,
    property_bindings: usize,
    clips: usize,
    clip_name_bytes: usize,
    clip_names: BTreeSet<String>,
    looping_clips: usize,
    states: usize,
    state_id_sum: i64,
    transitions: usize,
    event_id_sum: i64,
    transition_flags: BTreeMap<i64, usize>,
    selectors: usize,
    selector_anim_id_sum: i64,
    selector_offsets: BTreeMap<String, usize>,
    name_bytes: usize,
}

fn table(slice: &[(&str, usize)]) -> BTreeMap<String, usize> {
    slice.iter().map(|&(n, v)| (n.to_owned(), v)).collect()
}

/// One value an item holds, counted and read through its view.
fn read_value(found: &mut Found, value: havok_formats::Value<'_, '_>, object: bool) -> Result<(), Failure> {
    let Some(typed) = Typed::of(value) else {
        *found.untyped.entry(value.type_name().to_owned()).or_default() += 1;
        return Ok(());
    };
    if object {
        found.typed_objects += 1;
    } else {
        found.typed_elements += 1;
    }
    found.members_read += typed.read_members()?;
    let (members, unnamed) = typed.enum_members()?;
    found.enum_members += members;
    found.unnamed_enum_members += unnamed;
    match typed {
        Typed::ClipGenerator(clip) => {
            let name = clip.animation_name()?;
            found.clips += 1;
            found.clip_name_bytes += name.len();
            found.clip_names.insert(name);
            found.looping_clips += usize::from(clip.mode()?.name() == Some("MODE_LOOPING"));
        }
        Typed::StateMachineStateInfo(state) => {
            found.states += 1;
            found.state_id_sum += i64::from(state.state_id()?);
        }
        Typed::StateMachineTransitionInfo(transition) => {
            found.transitions += 1;
            found.event_id_sum += i64::from(transition.event_id()?);
            *found.transition_flags.entry(transition.flags()?.raw).or_default() += 1;
        }
        Typed::CustomManualSelectorGenerator(selector) => {
            found.selectors += 1;
            found.selector_anim_id_sum += i64::from(selector.anim_id()?);
            let offset = selector.offset_type()?;
            let name = offset.name().ok_or("an offset type the table does not name")?;
            *found.selector_offsets.entry(name.to_owned()).or_default() += 1;
        }
        _ => {}
    }
    Ok(())
}

fn read_file(found: &mut Found, name: &str, bytes: &[u8]) -> Result<(), Failure> {
    let tag = Tagfile::read(bytes, None).map_err(|e| format!("{name}: {e}"))?;
    let document = Document::read(&tag).map_err(|e| format!("{name}: {e}"))?;
    found.files += 1;
    found.counts.add(&document.class_counts()?);

    // every object is reachable from the root container through pointers
    let objects = document.objects()?;
    let reachable = document.reachable()?;
    assert_eq!(
        reachable.len(),
        objects.len(),
        "{name}: objects not reachable from the root container"
    );
    let offsets: BTreeSet<usize> = reachable.iter().map(|o| o.offset()).collect();
    assert_eq!(offsets.len(), reachable.len(), "{name}: an object listed twice");

    // every object and structure element of a class of the views, read through its view
    for (index, item) in tag.items.iter().enumerate().skip(1) {
        let array = tag.item(index)?;
        match item.kind {
            ItemKind::Object => read_value(found, array.get(0)?, true)?,
            ItemKind::Run if tag.format(item.type_index)? & 15 == 7 => {
                for i in 0..array.len() {
                    read_value(found, array.get(i)?, false)?;
                }
            }
            _ => {}
        }
    }

    match document.kind() {
        FileKind::Behavior => {
            found.behaviors += 1;
            let behavior = document.behavior().map_err(|e| format!("{name}: {e}"))?;
            let checked = behavior.check(&document).map_err(|e| format!("{name}: {e}"))?;
            let variables = behavior.variables()?;
            let events = behavior.events()?;
            assert_eq!(variables.len(), checked.variables, "{name}");
            assert_eq!(events.len(), checked.events, "{name}");
            found.variables += checked.variables;
            found.events += checked.events;
            found.graph_properties += checked.character_properties;
            found.variable_bindings += checked.variable_bindings;
            found.property_bindings += checked.property_bindings;
            found.name_bytes += variables.iter().map(|v| v.name.len()).sum::<usize>()
                + events.iter().map(|e| e.name.len()).sum::<usize>();
        }
        FileKind::Character => {
            found.characters += 1;
            found.character_properties += document.character_properties()?.len();
        }
        FileKind::Project => {
            found.projects += 1;
            let project = document.project_data()?;
            assert!(
                project.string_data()?.is_some(),
                "{name}: a project without string data"
            );
        }
    }
    Ok(())
}

fn run(game: Game, expected: &Expected) -> Result<(), Failure> {
    let corpus = Query::Oracle(OracleSet::Corpus, game, "havok-behavior");
    let Some(dir) = resolve(corpus) else {
        return Ok(());
    };
    let mut paths = std::fs::read_dir(&dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let paths: Vec<_> = paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "hkx"))
        .collect();
    expect_found(corpus, ".hkx files", paths.len());
    // manifest.tsv: a header and one row per binder member the files were found in
    let manifest = std::fs::read_to_string(dir.join("manifest.tsv"))?;
    assert_eq!(
        manifest.lines().skip(1).count(),
        expected.members,
        "binder members in the manifest"
    );
    let mut found = Found::default();
    for path in &paths {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        read_file(&mut found, &name, &std::fs::read(path)?)?;
    }

    assert_eq!(found.files, expected.files);
    assert_eq!(
        (found.behaviors, found.characters, found.projects),
        (expected.behaviors, expected.characters, expected.projects)
    );
    assert_eq!(found.counts.objects, table(expected.objects), "objects by class");
    assert_eq!(
        found.counts.elements,
        table(expected.elements),
        "structure elements by class"
    );
    // every class of the files but the root container and its variants is one of the views'
    assert_eq!(
        found.untyped,
        table(&[
            ("hkRootLevelContainer", expected.files),
            ("hkRootLevelContainer::NamedVariant", expected.files)
        ])
    );
    assert_eq!(found.typed_objects, expected.typed_objects);
    assert_eq!(found.typed_elements, expected.typed_elements);
    assert_eq!(
        found.members_read, expected.members_read,
        "members read through the views"
    );
    assert_eq!(found.enum_members, expected.enum_members);
    assert_eq!(found.unnamed_enum_members, expected.unnamed_enum_members);
    assert_eq!(found.variables, expected.variables);
    assert_eq!(found.events, expected.events);
    assert_eq!(found.graph_properties, expected.graph_properties);
    assert_eq!(found.character_properties, expected.character_properties);
    assert_eq!(found.variable_bindings, expected.variable_bindings);
    assert_eq!(found.property_bindings, expected.property_bindings);
    assert_eq!(found.clips, expected.clips);
    assert_eq!(found.clip_name_bytes, expected.clip_name_bytes);
    assert_eq!(found.clip_names.len(), expected.distinct_clip_names);
    assert_eq!(found.looping_clips, expected.looping_clips);
    assert_eq!(found.states, expected.states);
    assert_eq!(found.state_id_sum, expected.state_id_sum);
    assert_eq!(found.transitions, expected.transitions);
    assert_eq!(found.event_id_sum, expected.event_id_sum);
    assert_eq!(
        found.transition_flags.into_iter().collect::<Vec<_>>(),
        expected.transition_flags.to_vec()
    );
    assert_eq!(found.selectors, expected.selectors);
    assert_eq!(found.selector_anim_id_sum, expected.selector_anim_id_sum);
    assert_eq!(found.selector_offsets, table(expected.selector_offsets));
    assert_eq!(found.name_bytes, expected.name_bytes);
    // the view's class names are the classes the files hold
    for name in found.counts.objects.keys().chain(found.counts.elements.keys()) {
        if let Some(class) = Class::from_name(name) {
            assert_eq!(class.name(), name);
        }
    }
    Ok(())
}

#[test]
fn every_behaviour_tagfile_of_sekiro_reads_through_the_views() -> Result<(), Failure> {
    run(Game::Sekiro, &SEKIRO)
}

#[test]
fn every_behaviour_tagfile_of_elden_ring_reads_through_the_views() -> Result<(), Failure> {
    run(Game::Elden, &ELDEN)
}
