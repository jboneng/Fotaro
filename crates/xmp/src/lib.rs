//! XMP sidecars, read and updated the way Lightroom Classic, Bridge and darktable expect.
//!
//! [`XmpDoc`] edits only the properties this app manages ([`XmpData`]) and keeps
//! everything else (develop settings from LrC/darktable, history, unknown namespaces)
//! byte-for-byte where possible. Sidecar naming follows [`sidecar_path`].

mod dom;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dom::{Element, Node};

/// Namespace URIs.
pub mod ns {
    pub const X: &str = "adobe:ns:meta/";
    pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    pub const XMP: &str = "http://ns.adobe.com/xap/1.0/";
    pub const DC: &str = "http://purl.org/dc/elements/1.1/";
    pub const LR: &str = "http://ns.adobe.com/lightroom/1.0/";
    pub const TIFF: &str = "http://ns.adobe.com/tiff/1.0/";
    /// Camera Raw / Lightroom develop settings (read from Lightroom presets).
    pub const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
    /// This app's own properties (pick/reject flags have no standard XMP property;
    /// LrC keeps them in its catalog only).
    pub const PHOTOLIBRARY: &str = "https://github.com/jboneng/ImageLibrary/ns/xmp/1.0/";
}

fn preferred_prefix(uri: &str) -> &'static str {
    match uri {
        ns::RDF => "rdf",
        ns::XMP => "xmp",
        ns::DC => "dc",
        ns::LR => "lr",
        ns::TIFF => "tiff",
        ns::PHOTOLIBRARY => "photolibrary",
        _ => "ns",
    }
}

/// The metadata this app reads from and writes to sidecars. `None` means "not
/// present" when reading and "leave untouched" when writing; an empty string or
/// list removes the property.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct XmpData {
    /// `xmp:Rating`: 0–5 stars (Bridge writes -1 for "rejected").
    pub rating: Option<i8>,
    /// `xmp:Label`: LrC's label names ("Red", "Yellow", "Green", "Blue", "Purple").
    pub label: Option<String>,
    /// This app's pick flag: 1 picked, 0 unflagged, -1 rejected.
    pub pick: Option<i8>,
    /// `tiff:Orientation`: EXIF orientation code of the photo as displayed.
    pub orientation: Option<u8>,
    /// Keyword paths from `lr:hierarchicalSubject` (`Places|Norway|Oslo`), or flat
    /// `dc:subject` keywords when there is no hierarchy.
    pub keywords: Option<Vec<Vec<String>>>,
    /// `dc:title`, `dc:description` (LrC's Caption), `dc:creator`, `dc:rights` (Copyright).
    pub title: Option<String>,
    pub caption: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    /// This app's Develop settings (JSON) as `photolibrary:Develop`. Our own property:
    /// LrC's `crs:` settings mean something else to its renderer.
    pub develop: Option<String>,
}

type Scope = Vec<(String, String)>;

fn split(qname: &str) -> (&str, &str) {
    qname.split_once(':').unwrap_or(("", qname))
}

fn with_declarations(scope: &Scope, e: &Element) -> Scope {
    let mut scope = scope.clone();
    for (name, value) in &e.attrs {
        if name == "xmlns" {
            scope.push((String::new(), value.clone()));
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            scope.push((prefix.to_owned(), value.clone()));
        }
    }
    scope
}

fn resolve<'a>(scope: &'a Scope, prefix: &str) -> Option<&'a str> {
    scope.iter().rev().find(|(p, _)| p == prefix).map(|(_, uri)| uri.as_str())
}

fn prefix_for<'a>(scope: &'a Scope, uri: &str) -> Option<&'a str> {
    // The innermost binding of a prefix wins, so check it still points at `uri`.
    scope
        .iter()
        .rev()
        .find(|(p, u)| u == uri && !p.is_empty() && resolve(scope, p) == Some(uri))
        .map(|(p, _)| p.as_str())
}

/// Does element name `qname` mean `{uri}local` in `scope`?
fn is_element(scope: &Scope, qname: &str, uri: &str, local: &str) -> bool {
    let (prefix, l) = split(qname);
    l == local && resolve(scope, prefix) == Some(uri)
}

/// Same for attributes: unprefixed attributes are in no namespace.
fn is_attr(scope: &Scope, qname: &str, uri: &str, local: &str) -> bool {
    let (prefix, l) = split(qname);
    !prefix.is_empty() && prefix != "xmlns" && l == local && resolve(scope, prefix) == Some(uri)
}

/// An `rdf:Description`: where it is in the tree, and the namespaces in scope there.
struct Description {
    path: Vec<usize>,
    scope: Scope,
}

fn find_descriptions(nodes: &[Node], path: &mut Vec<usize>, scope: &Scope, out: &mut Vec<Description>) {
    for (i, node) in nodes.iter().enumerate() {
        let Node::Element(e) = node else { continue };
        let scope = with_declarations(scope, e);
        path.push(i);
        if is_element(&scope, &e.name, ns::RDF, "Description") {
            // Nested Descriptions are structured property values, not top-level ones.
            out.push(Description { path: path.clone(), scope });
        } else {
            find_descriptions(&e.children, path, &scope, out);
        }
        path.pop();
    }
}

fn element_at<'a>(nodes: &'a [Node], path: &[usize]) -> &'a Element {
    let Node::Element(e) = &nodes[path[0]] else { unreachable!("paths point at elements") };
    if path.len() == 1 { e } else { element_at(&e.children, &path[1..]) }
}

fn element_at_mut<'a>(nodes: &'a mut [Node], path: &[usize]) -> &'a mut Element {
    let Node::Element(e) = &mut nodes[path[0]] else { unreachable!("paths point at elements") };
    if path.len() == 1 { e } else { element_at_mut(&mut e.children, &path[1..]) }
}

enum Value {
    Simple(String),
    /// Language alternative; written as the `x-default` item.
    Alt(String),
    Seq(Vec<String>),
    Bag(Vec<String>),
}

/// A parsed XMP packet that can be read and updated in place.
#[derive(Debug, Clone)]
pub struct XmpDoc {
    nodes: Vec<Node>,
}

const SKELETON: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Fotaro\">\n \
     <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
     <rdf:Description rdf:about=\"\"/>\n \
     </rdf:RDF>\n</x:xmpmeta>\n";

impl Default for XmpDoc {
    fn default() -> Self {
        XmpDoc { nodes: dom::parse(SKELETON).expect("skeleton is valid XMP") }
    }
}

impl XmpDoc {
    pub fn parse(xml: &str) -> Result<Self> {
        // A UTF-8 byte order mark is legal before the packet.
        let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
        let doc = XmpDoc { nodes: dom::parse(xml)? };
        anyhow::ensure!(!doc.descriptions().is_empty(), "no rdf:Description found; not an XMP packet");
        Ok(doc)
    }

    fn descriptions(&self) -> Vec<Description> {
        let mut out = Vec::new();
        find_descriptions(&self.nodes, &mut Vec::new(), &Scope::new(), &mut out);
        out
    }

    /// The first value of a property in any Description: a simple value, or the items
    /// of an `rdf:Alt` (x-default first), `rdf:Seq` or `rdf:Bag`.
    fn get(&self, uri: &str, local: &str) -> Option<Vec<String>> {
        for d in self.descriptions() {
            let e = element_at(&self.nodes, &d.path);
            if let Some((_, v)) = e.attrs.iter().find(|(n, _)| is_attr(&d.scope, n, uri, local)) {
                return Some(vec![v.clone()]);
            }
            for child in e.elements() {
                let scope = with_declarations(&d.scope, child);
                if !is_element(&scope, &child.name, uri, local) {
                    continue;
                }
                let container = child.elements().find(|c| {
                    let s = with_declarations(&scope, c);
                    ["Alt", "Seq", "Bag"].iter().any(|k| is_element(&s, &c.name, ns::RDF, k))
                });
                // A structure (a nested Description): its simple fields as `Name=value`.
                let structure = child.elements().find(|c| {
                    is_element(&with_declarations(&scope, c), &c.name, ns::RDF, "Description")
                });
                if container.is_none()
                    && let Some(st) = structure
                {
                    let s = with_declarations(&scope, st);
                    let fields = st
                        .attrs
                        .iter()
                        .filter(|(n, _)| !n.starts_with("xmlns") && !is_attr(&s, n, ns::RDF, split(n).1))
                        .map(|(n, v)| format!("{}={v}", split(n).1));
                    let elems = st.elements().filter(|c| c.elements().next().is_none()).map(|c| {
                        format!("{}={}", split(&c.name).1, c.text())
                    });
                    return Some(fields.chain(elems).collect());
                }
                return Some(match container {
                    Some(c) => {
                        let s = with_declarations(&scope, c);
                        let mut items: Vec<(bool, String)> = c
                            .elements()
                            .filter(|li| is_element(&with_declarations(&s, li), &li.name, ns::RDF, "li"))
                            .map(|li| (li.attr("xml:lang") == Some("x-default"), li.text()))
                            .collect();
                        items.sort_by_key(|(default, _)| !default); // stable: x-default first
                        items.into_iter().map(|(_, t)| t).collect()
                    }
                    None => vec![child.text()],
                });
            }
        }
        None
    }

    /// Every top-level property in namespace `uri` (by local name, first one wins), each
    /// as [`Self::get`] gives it. Structured values (nested Descriptions) come out as their
    /// text, so callers can tell they are there.
    pub fn properties(&self, uri: &str) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for d in self.descriptions() {
            let e = element_at(&self.nodes, &d.path);
            for (n, _) in &e.attrs {
                if is_attr(&d.scope, n, uri, split(n).1) {
                    names.push(split(n).1.to_owned());
                }
            }
            for child in e.elements() {
                let scope = with_declarations(&d.scope, child);
                let (prefix, local) = split(&child.name);
                if resolve(&scope, prefix) == Some(uri) {
                    names.push(local.to_owned());
                }
            }
        }
        for name in names {
            if !out.iter().any(|(n, _)| *n == name)
                && let Some(v) = self.get(uri, &name)
            {
                out.push((name, v));
            }
        }
        out
    }

    fn get_one(&self, uri: &str, local: &str) -> Option<String> {
        self.get(uri, local).and_then(|v| v.into_iter().next())
    }

    /// Remove a property (attribute or element form) from every Description.
    fn remove(&mut self, uri: &str, local: &str) {
        for d in self.descriptions() {
            let e = element_at_mut(&mut self.nodes, &d.path);
            e.attrs.retain(|(n, _)| !is_attr(&d.scope, n, uri, local));
            let mut i = 0;
            while i < e.children.len() {
                let hit = matches!(&e.children[i], Node::Element(c)
                    if is_element(&with_declarations(&d.scope, c), &c.name, uri, local));
                if hit {
                    e.children.remove(i);
                    // Drop the indentation in front of it too, so lines don't pile up.
                    if i > 0 && matches!(&e.children[i - 1], Node::Text(t) if t.trim().is_empty()) {
                        e.children.remove(i - 1);
                        i -= 1;
                    }
                } else {
                    i += 1;
                }
            }
        }
    }

    /// Replace a property everywhere with `value` (`None` removes it).
    fn set(&mut self, uri: &str, local: &str, value: Option<Value>) {
        self.remove(uri, local);
        let Some(value) = value else { return };
        let descriptions = self.descriptions();
        let main = &descriptions[0];
        let main_path = main.path.clone();
        let rdf = prefix_for(&main.scope, ns::RDF).unwrap_or("rdf").to_owned();

        // Declare the property's namespace on the Description if it isn't in scope.
        let prefix = match prefix_for(&main.scope, uri) {
            Some(p) => p.to_owned(),
            None => {
                let base = preferred_prefix(uri);
                let mut p = base.to_owned();
                let mut n = 1;
                while resolve(&main.scope, &p).is_some() {
                    p = format!("{base}{n}");
                    n += 1;
                }
                element_at_mut(&mut self.nodes, &main_path)
                    .attrs
                    .push((format!("xmlns:{p}"), uri.to_owned()));
                p
            }
        };

        let li = |text: String, lang: bool| {
            let mut e = Element::new(format!("{rdf}:li"));
            if lang {
                e.attrs.push(("xml:lang".into(), "x-default".into()));
            }
            e.children.push(Node::Text(text));
            Node::Element(e)
        };
        let container = |kind: &str, items: Vec<Node>| {
            let mut c = Element::new(format!("{rdf}:{kind}"));
            c.children = items;
            Node::Element(c)
        };
        let mut prop = Element::new(format!("{prefix}:{local}"));
        prop.children.push(match value {
            Value::Simple(v) => Node::Text(v),
            Value::Alt(v) => container("Alt", vec![li(v, true)]),
            Value::Seq(items) => container("Seq", items.into_iter().map(|t| li(t, false)).collect()),
            Value::Bag(items) => container("Bag", items.into_iter().map(|t| li(t, false)).collect()),
        });

        let e = element_at_mut(&mut self.nodes, &main_path);
        // Reuse the existing child indentation; keep the closing indentation last.
        let indent = e
            .children
            .iter()
            .find_map(|n| match n {
                Node::Text(t) if t.trim().is_empty() && t.contains('\n') => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "\n   ".to_owned());
        let at = match e.children.last() {
            Some(Node::Text(t)) if t.trim().is_empty() => e.children.len() - 1,
            _ => {
                e.children.push(Node::Text("\n  ".into()));
                e.children.len() - 1
            }
        };
        e.children.splice(at..at, [Node::Text(indent), Node::Element(prop)]);
    }

    /// The managed properties present in this packet.
    pub fn data(&self) -> XmpData {
        let number = |uri, local| self.get_one(uri, local).and_then(|v| v.trim().parse::<f32>().ok());
        let text = |uri, local| self.get_one(uri, local).filter(|v| !v.trim().is_empty());
        let keywords = match self.get(ns::LR, "hierarchicalSubject") {
            Some(paths) => Some(
                paths
                    .iter()
                    .map(|p| p.split('|').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect())
                    .filter(|p: &Vec<String>| !p.is_empty())
                    .collect(),
            ),
            None => self.get(ns::DC, "subject").map(|v| {
                v.into_iter().filter(|k| !k.trim().is_empty()).map(|k| vec![k.trim().to_owned()]).collect()
            }),
        };
        XmpData {
            rating: number(ns::XMP, "Rating").map(|r| r.round().clamp(-1.0, 5.0) as i8),
            label: text(ns::XMP, "Label"),
            pick: number(ns::PHOTOLIBRARY, "Pick").map(|p| p.round().clamp(-1.0, 1.0) as i8),
            orientation: number(ns::TIFF, "Orientation").map(|o| o as u8).filter(|o| (1..=8).contains(o)),
            keywords,
            title: text(ns::DC, "title"),
            caption: text(ns::DC, "description"),
            creator: self.get(ns::DC, "creator").map(|v| v.join("; ")).filter(|v| !v.trim().is_empty()),
            copyright: text(ns::DC, "rights"),
            develop: text(ns::PHOTOLIBRARY, "Develop"),
        }
    }

    /// Write every `Some` field of `data` (empty strings / lists remove the property).
    pub fn apply(&mut self, data: &XmpData) {
        let nonempty = |s: &String| !s.trim().is_empty();
        if let Some(r) = data.rating {
            self.set(ns::XMP, "Rating", Some(Value::Simple(r.to_string())));
        }
        if let Some(l) = &data.label {
            self.set(ns::XMP, "Label", nonempty(l).then(|| Value::Simple(l.clone())));
        }
        if let Some(p) = data.pick {
            self.set(ns::PHOTOLIBRARY, "Pick", (p != 0).then(|| Value::Simple(p.to_string())));
        }
        if let Some(o) = data.orientation {
            self.set(ns::TIFF, "Orientation", Some(Value::Simple(o.to_string())));
        }
        if let Some(paths) = &data.keywords {
            let paths: Vec<&Vec<String>> = paths.iter().filter(|p| !p.is_empty()).collect();
            // dc:subject gets every level (LrC's "include parent keywords"), deduplicated.
            let mut flat: Vec<String> = Vec::new();
            for k in paths.iter().flat_map(|p| p.iter()) {
                if !flat.contains(k) {
                    flat.push(k.clone());
                }
            }
            let hierarchical: Vec<String> = paths.iter().map(|p| p.join("|")).collect();
            let any = !paths.is_empty();
            self.set(ns::DC, "subject", any.then_some(Value::Bag(flat)));
            self.set(ns::LR, "hierarchicalSubject", any.then_some(Value::Bag(hierarchical)));
        }
        for (local, value) in
            [("title", &data.title), ("description", &data.caption), ("rights", &data.copyright)]
        {
            if let Some(v) = value {
                self.set(ns::DC, local, nonempty(v).then(|| Value::Alt(v.clone())));
            }
        }
        if let Some(d) = &data.develop {
            self.set(ns::PHOTOLIBRARY, "Develop", nonempty(d).then(|| Value::Simple(d.clone())));
        }
        if let Some(c) = &data.creator {
            self.set(ns::DC, "creator", nonempty(c).then(|| Value::Seq(vec![c.clone()])));
        }
    }

    pub fn has_orientation(&self) -> bool {
        self.get_one(ns::TIFF, "Orientation").is_some()
    }

    pub fn to_xml(&self) -> String {
        dom::write(&self.nodes)
    }
}

/// Where a photo's sidecar lives. RAW files use `IMG_1234.xmp` (LrC, Bridge, Capture
/// One; darktable reads it on import). Other files use `IMG_1234.tif.xmp` (darktable,
/// digiKam), so a RAW+JPEG pair never shares one sidecar.
pub fn sidecar_path(photo: &Path, raw: bool) -> PathBuf {
    if raw {
        photo.with_extension("xmp")
    } else {
        let mut name = photo.file_name().unwrap_or_default().to_os_string();
        name.push(".xmp");
        photo.with_file_name(name)
    }
}

fn mtime_ms(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// A sidecar read from disk.
#[derive(Debug, Clone)]
pub struct Sidecar {
    pub data: XmpData,
    pub mtime_ms: i64,
}

/// Read a photo's sidecar if it has one.
pub fn read_sidecar(photo: &Path, raw: bool) -> Result<Option<Sidecar>> {
    let path = sidecar_path(photo, raw);
    let xml = match std::fs::read_to_string(&path) {
        Ok(x) => x,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let doc = XmpDoc::parse(&xml).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(Sidecar { data: doc.data(), mtime_ms: mtime_ms(&path).unwrap_or(0) }))
}

/// Update (or create) a photo's sidecar with `data`, keeping everything else in it.
/// `orientation` is written only if `data.orientation` is set and either the photo is
/// rotated or the sidecar already had one (`write_orientation(existing_has_one)`).
/// Refuses to touch a sidecar it cannot parse. Returns the new modification time.
pub fn write_sidecar(
    photo: &Path,
    raw: bool,
    data: &XmpData,
    write_orientation: impl FnOnce(bool) -> bool,
) -> Result<i64> {
    let path = sidecar_path(photo, raw);
    let mut doc = match std::fs::read_to_string(&path) {
        Ok(xml) => XmpDoc::parse(&xml)
            .with_context(|| format!("{} is not valid XMP; leaving it alone", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => XmpDoc::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut data = data.clone();
    if !write_orientation(doc.has_orientation()) {
        data.orientation = None;
    }
    doc.apply(&data);
    // Write next to it and rename, so a crash never leaves a half-written sidecar.
    let tmp = path.with_extension("xmp.tmp");
    std::fs::write(&tmp, doc.to_xml()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(mtime_ms(&path).unwrap_or(0))
}

#[cfg(test)]
mod tests;
