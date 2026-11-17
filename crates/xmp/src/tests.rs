use super::*;

/// The shape of a Lightroom Classic sidecar for a raw file: simple properties as
/// attributes, develop settings (crs:) that must survive our edits.
const LRC: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="4"
   xmp:Label="Red"
   tiff:Orientation="6"
   crs:Version="15.0"
   crs:Exposure2012="+0.35">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>Norway</rdf:li>
     <rdf:li>Trondheim</rdf:li>
    </rdf:Bag>
   </dc:subject>
   <lr:hierarchicalSubject>
    <rdf:Bag>
     <rdf:li>Places|Norway|Trondheim</rdf:li>
    </rdf:Bag>
   </lr:hierarchicalSubject>
   <dc:title>
    <rdf:Alt>
     <rdf:li xml:lang="de">Brücke</rdf:li>
     <rdf:li xml:lang="x-default">Bridge &amp; river</rdf:li>
    </rdf:Alt>
   </dc:title>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

/// darktable's style: xpacket wrapper, element-form rating, its own history.
const DARKTABLE: &str = r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5"
   darktable:auto_presets_applied="1">
   <xmp:Rating>2</xmp:Rating>
   <dc:subject>
    <rdf:Bag>
     <rdf:li>cat</rdf:li>
     <rdf:li>film</rdf:li>
    </rdf:Bag>
   </dc:subject>
   <darktable:history>
    <rdf:Seq>
     <rdf:li darktable:operation="exposure" darktable:enabled="1"/>
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

fn path(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| s.to_string()).collect()
}

#[test]
fn reads_lightroom_sidecars() {
    let d = XmpDoc::parse(LRC).unwrap().data();
    assert_eq!(d.rating, Some(4));
    assert_eq!(d.label.as_deref(), Some("Red"));
    assert_eq!(d.orientation, Some(6));
    assert_eq!(d.keywords, Some(vec![path(&["Places", "Norway", "Trondheim"])]));
    assert_eq!(d.title.as_deref(), Some("Bridge & river"), "x-default wins over other languages");
    assert_eq!(d.pick, None);
}

#[test]
fn reads_darktable_sidecars_with_flat_keywords() {
    let d = XmpDoc::parse(DARKTABLE).unwrap().data();
    assert_eq!(d.rating, Some(2));
    assert_eq!(d.keywords, Some(vec![path(&["cat"]), path(&["film"])]));
}

#[test]
fn updates_keep_foreign_content() {
    let mut doc = XmpDoc::parse(LRC).unwrap();
    doc.apply(&XmpData {
        rating: Some(2),
        label: Some(String::new()),
        pick: Some(1),
        keywords: Some(vec![path(&["Places", "Norway", "Oslo"]), path(&["Cat"])]),
        title: Some(String::new()),
        caption: Some("A <caption>".into()),
        creator: Some("J. B.".into()),
        ..Default::default()
    });
    let xml = doc.to_xml();
    // LrC's develop settings and everything we don't manage are untouched.
    for kept in
        ["crs:Exposure2012=\"+0.35\"", "crs:Version=\"15.0\"", "<rdf:li>255, 255</rdf:li>", "x:xmptk=\"Adobe"]
    {
        assert!(xml.contains(kept), "lost {kept}:\n{xml}");
    }
    assert!(!xml.contains("xmp:Rating=\""), "the old attribute was replaced:\n{xml}");
    assert_eq!(xml.matches("<xmp:Rating>").count(), 1, "{xml}");
    let d = XmpDoc::parse(&xml).unwrap().data();
    assert_eq!(d.rating, Some(2));
    assert_eq!(d.label, None);
    assert_eq!(d.pick, Some(1));
    assert_eq!(d.orientation, Some(6), "not managed in this update, so untouched");
    assert_eq!(d.keywords, Some(vec![path(&["Places", "Norway", "Oslo"]), path(&["Cat"])]));
    assert_eq!(d.title, None);
    assert_eq!(d.caption.as_deref(), Some("A <caption>"));
    assert_eq!(d.creator.as_deref(), Some("J. B."));
    // dc:subject lists every level once, for apps without hierarchy support.
    let flat = XmpDoc::parse(&xml).unwrap().get(ns::DC, "subject").unwrap();
    assert_eq!(flat, ["Places", "Norway", "Oslo", "Cat"]);
}

#[test]
fn darktable_history_and_packet_wrapper_survive() {
    let mut doc = XmpDoc::parse(DARKTABLE).unwrap();
    doc.apply(&XmpData { rating: Some(5), keywords: Some(vec![]), ..Default::default() });
    let xml = doc.to_xml();
    assert!(xml.starts_with("<?xpacket begin="));
    assert!(xml.trim_end().ends_with("<?xpacket end=\"w\"?>"));
    assert!(xml.contains("darktable:operation=\"exposure\""));
    let d = XmpDoc::parse(&xml).unwrap().data();
    assert_eq!(d.rating, Some(5));
    assert_eq!(d.keywords, None, "an empty keyword list removes dc:subject");
}

#[test]
fn uses_whatever_prefix_the_file_binds() {
    let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description xmlns:xap="http://ns.adobe.com/xap/1.0/" xmlns:xmp="urn:not-xmp" xap:Rating="3" xmp:Rating="9"/>
      </rdf:RDF></x:xmpmeta>"#;
    let mut doc = XmpDoc::parse(xml).unwrap();
    assert_eq!(doc.data().rating, Some(3), "xmp: is bound to another namespace here");
    doc.apply(&XmpData { rating: Some(1), ..Default::default() });
    let out = doc.to_xml();
    assert!(out.contains("<xap:Rating>1</xap:Rating>"), "{out}");
    assert!(out.contains("xmp:Rating=\"9\""), "a foreign property with a look-alike name is kept");
    assert_eq!(XmpDoc::parse(&out).unwrap().data().rating, Some(1));
}

#[test]
fn new_documents_round_trip() {
    let mut doc = XmpDoc::default();
    let data = XmpData {
        rating: Some(3),
        label: Some("Green".into()),
        pick: Some(-1),
        orientation: Some(8),
        keywords: Some(vec![path(&["Film", "Fuji C200"])]),
        title: Some("Hegra".into()),
        caption: Some("Line 1\nLine 2".into()),
        creator: Some("Me".into()),
        copyright: Some("© 2026".into()),
        develop: Some(r#"{"exposure":1.0}"#.into()),
    };
    doc.apply(&data);
    assert_eq!(XmpDoc::parse(&doc.to_xml()).unwrap().data(), data);
}

#[test]
fn not_xmp_is_an_error() {
    assert!(XmpDoc::parse("<html><body/></html>").is_err());
    assert!(XmpDoc::parse("not xml <").is_err());
}

#[test]
fn sidecar_names() {
    let p = Path::new("/photos/IMG_1.NEF");
    assert_eq!(sidecar_path(p, true), Path::new("/photos/IMG_1.xmp"));
    assert_eq!(sidecar_path(Path::new("/photos/IMG_1.tif"), false), Path::new("/photos/IMG_1.tif.xmp"));
}

#[test]
fn sidecar_files_are_updated_in_place_and_garbage_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let photo = tmp.path().join("IMG_1.NEF");
    std::fs::write(tmp.path().join("IMG_1.xmp"), LRC).unwrap();

    let data = XmpData { rating: Some(1), orientation: Some(1), ..Default::default() };
    // Not rotated, but the sidecar already has an orientation: keep it in sync.
    write_sidecar(&photo, true, &data, |had| had).unwrap();
    let back = read_sidecar(&photo, true).unwrap().unwrap();
    assert_eq!((back.data.rating, back.data.orientation), (Some(1), Some(1)));
    assert!(std::fs::read_to_string(tmp.path().join("IMG_1.xmp")).unwrap().contains("crs:Exposure2012"));

    // A new sidecar for an unrotated photo gets no orientation at all.
    let jpg = tmp.path().join("IMG_1.JPG");
    write_sidecar(&jpg, false, &data, |had| had).unwrap();
    assert_eq!(read_sidecar(&jpg, false).unwrap().unwrap().data.orientation, None);

    assert!(read_sidecar(&tmp.path().join("none.NEF"), true).unwrap().is_none());
    let broken = tmp.path().join("bad.NEF");
    std::fs::write(tmp.path().join("bad.xmp"), "<not xmp").unwrap();
    assert!(write_sidecar(&broken, true, &data, |_| true).is_err());
    assert_eq!(std::fs::read_to_string(tmp.path().join("bad.xmp")).unwrap(), "<not xmp");
}

#[test]
fn develop_settings_round_trip_and_keep_lightroom_settings() {
    let json = r#"{"exposure":0.35,"wb":"Tungsten","note":"a < b & \"c\""}"#;
    let mut doc = XmpDoc::parse(LRC).unwrap();
    doc.apply(&XmpData { develop: Some(json.into()), ..Default::default() });
    let xml = doc.to_xml();
    assert!(xml.contains("crs:Exposure2012=\"+0.35\""), "LrC's settings stay: {xml}");
    let back = XmpDoc::parse(&xml).unwrap().data();
    assert_eq!(back.develop.as_deref(), Some(json));
    assert_eq!(back.rating, Some(4));

    // An empty value removes it.
    let mut doc = XmpDoc::parse(&xml).unwrap();
    doc.apply(&XmpData { develop: Some(String::new()), ..Default::default() });
    assert_eq!(XmpDoc::parse(&doc.to_xml()).unwrap().data().develop, None);
}
