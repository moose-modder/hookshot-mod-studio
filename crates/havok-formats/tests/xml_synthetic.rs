//! The XML form of a tagfile's objects on a synthetic file: the value elements for each kind of member, the numbering of
//! objects by first reference, the identities of types, and the differences between two files. No game payload is
//! embedded.
mod graphbuilder;

use graphbuilder::{Graph, Schema, Ty, V, arr, build, floats, ints, r, s, vec4};
use havok_formats::Tagfile;
use havok_formats::xml::{Element, diff, diff_tagfiles, identity_form, latin1_strings, objects};

fn schema() -> Schema {
    Schema::from([(
        "Node",
        vec![
            ("count", Ty::I32),
            ("ratio", Ty::F32),
            ("ok", Ty::Bool),
            ("name", Ty::Str),
            ("next", Ty::Ptr("Node")),
            ("values", arr(Ty::I32)),
            ("v", vec4()),
            ("tag", Ty::U8),
        ],
    )])
}

fn graph(count: i64, name: &str) -> Graph {
    vec![
        (
            "Node",
            r(vec![
                ("count", V::I(count)),
                ("ratio", V::F(0.5)),
                ("ok", V::I(1)),
                ("name", s(name)),
                ("next", V::Obj(1)),
                ("values", ints(&[1, 2])),
                ("v", floats(&[1.0, 0.0, 0.0, 2.0])),
                ("tag", V::I(200)),
            ]),
        ),
        ("Node", r(vec![("count", V::I(7)), ("next", V::Obj(0))])),
    ]
}

/// The objects of the graph above with count -3 and the name `a&b`, as the format of the module documentation writes them
/// (the expected text was written from the rules, not from this crate's output).
const EXPECTED: &str = r##"<hktagfile version="3">
  <object id="object1" typeid="Node">
    <record>
      <field name="count"><integer value="-3" /></field>
      <field name="ratio"><real hex="#3fe0000000000000" /></field>
      <field name="ok"><bool value="true" /></field>
      <field name="name"><string value="a&amp;b" /></field>
      <field name="next"><pointer id="object2" /></field>
      <field name="values"><array count="2" elementtypeid="hkInt32"><integer value="1" /><integer value="2" /></array></field>
      <field name="v"><array count="4" elementtypeid="hkReal"><real hex="#3ff0000000000000" /><real hex="#0" /><real hex="#0" /><real hex="#4000000000000000" /></array></field>
      <field name="tag"><integer value="200" /></field>
    </record>
  </object>
  <object id="object2" typeid="Node">
    <record>
      <field name="count"><integer value="7" /></field>
      <field name="ratio"><real hex="#0" /></field>
      <field name="ok"><bool value="false" /></field>
      <field name="name"><string value="" /></field>
      <field name="next"><pointer id="object1" /></field>
      <field name="values"><array count="0" elementtypeid="hkInt32" /></field>
      <field name="v"><array count="4" elementtypeid="hkReal"><real hex="#0" /><real hex="#0" /><real hex="#0" /><real hex="#0" /></array></field>
      <field name="tag"><integer value="0" /></field>
    </record>
  </object>
</hktagfile>"##;

#[test]
fn a_tagfiles_objects_are_written_as_the_format_says() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = build(b"20180100", &schema(), &graph(-3, "a&b"));
    let tag = Tagfile::read(&bytes, None)?;
    let written = objects(&tag)?;
    assert_eq!(written, Element::parse(EXPECTED)?);
    // the text writes back to the same tree
    assert_eq!(Element::parse(&written.write())?, written);
    // a document of the same objects written by other software, with numbered types and decimal renderings, reads as the same tree
    let other = Element::parse(
        r##"<hktagfile version="3">
          <type id="type1"><name value="Node" /></type>
          <type id="type2"><name value="hkInt32" /></type>
          <type id="type3"><name value="hkReal" /></type>
          <object id="object1" typeid="type1"><record>
            <field name="count"><integer value="-3" /></field>
            <field name="ratio"><real dec="0.5" hex="#3fe0000000000000" /></field>
            <field name="ok"><bool value="true" /></field>
            <field name="name"><string value="a&amp;b" /></field>
            <field name="next"><pointer id="object2" /></field>
            <field name="values"><array count="2" elementtypeid="type2"><integer value="1" /><integer value="2" /></array></field>
            <field name="v"><array count="4" elementtypeid="type3"><real dec="1" hex="#3ff0000000000000" /><real dec="0" hex="#0" /><real dec="0" hex="#0" /><real dec="2" hex="#4000000000000000" /></array></field>
            <field name="tag"><integer value="200" /></field>
          </record></object>
          <object id="object2" typeid="type1"><record>
            <field name="count"><integer value="7" /></field>
            <field name="ratio"><real dec="0" hex="#0" /></field>
            <field name="ok"><bool value="false" /></field>
            <field name="name"><string value="" /></field>
            <field name="next"><pointer id="object1" /></field>
            <field name="values"><array count="0" elementtypeid="type2" /></field>
            <field name="v"><array count="4" elementtypeid="type3"><real hex="#0" /><real hex="#0" /><real hex="#0" /><real hex="#0" /></array></field>
            <field name="tag"><integer value="0" /></field>
          </record></object>
        </hktagfile>"##,
    )?;
    assert!(diff(&written, &identity_form(&other)?, 10).is_empty());
    Ok(())
}

#[test]
fn two_files_differ_where_their_values_differ() -> Result<(), Box<dyn std::error::Error>> {
    let a = build(b"20180100", &schema(), &graph(-3, "a&b"));
    let b = build(b"20180100", &schema(), &graph(-4, "a&b"));
    let (ta, tb) = (Tagfile::read(&a, None)?, Tagfile::read(&b, None)?);
    assert!(diff_tagfiles(&ta, &ta, 10)?.is_empty());
    let found = diff_tagfiles(&ta, &tb, 10)?;
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].path,
        "hktagfile/object(object1)[0]/record[0]/field(count)[0]/integer[0]"
    );
    assert_eq!(found[0].left, "integer value=\"-3\"");
    assert_eq!(found[0].right, "integer value=\"-4\"");
    Ok(())
}

#[test]
fn a_non_ascii_string_is_one_character_a_byte_in_the_latin1_view() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = build(b"20180100", &schema(), &graph(1, "é"));
    let tag = Tagfile::read(&bytes, None)?;
    let written = objects(&tag)?;
    let mark = |tree: &Element| {
        tree.children
            .first()
            .and_then(|o| o.children.first())
            .and_then(|record| record.children.get(3))
            .and_then(|field| field.children.first())
            .and_then(|string| string.attribute("value"))
            .map(str::to_owned)
    };
    assert_eq!(mark(&written).as_deref(), Some("é"));
    assert_eq!(mark(&latin1_strings(&written)).as_deref(), Some("\u{c3}\u{a9}"));
    Ok(())
}
