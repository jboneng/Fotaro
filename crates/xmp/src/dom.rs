//! A minimal XML tree that round-trips what it does not understand.
//!
//! Element and attribute names are kept exactly as written (with their prefixes),
//! comments, processing instructions (`<?xpacket ...?>`) and whitespace are kept,
//! so updating a few properties leaves the rest of another app's sidecar untouched.

use anyhow::{Context, Result};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    Element(Element),
    /// Character data, unescaped.
    Text(String),
    /// Comments, processing instructions, declarations, CDATA: written back verbatim.
    Raw(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Element {
    /// Qualified name as written, e.g. `xmp:Rating`.
    pub name: String,
    /// `(qualified name, unescaped value)` in document order.
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    pub fn new(name: impl Into<String>) -> Self {
        Element { name: name.into(), attrs: Vec::new(), children: Vec::new() }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    /// Concatenated text of direct text children.
    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|n| match n {
                Node::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }
}

fn utf8(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).context("XMP is not valid UTF-8")
}

fn element(start: &BytesStart) -> Result<Element> {
    let mut e = Element::new(utf8(start.name().as_ref())?);
    for attr in start.attributes() {
        let attr = attr.context("malformed XML attribute")?;
        let value = attr.unescape_value().context("malformed XML attribute value")?;
        e.attrs.push((utf8(attr.key.as_ref())?.to_owned(), value.into_owned()));
    }
    Ok(e)
}

/// Parse a document into its top-level nodes.
pub(crate) fn parse(xml: &str) -> Result<Vec<Node>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut top: Vec<Node> = Vec::new();
    let mut stack: Vec<Element> = Vec::new();
    let push = |stack: &mut Vec<Element>, top: &mut Vec<Node>, node: Node| match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => top.push(node),
    };
    loop {
        let event =
            reader.read_event().with_context(|| format!("XML error at byte {}", reader.error_position()))?;
        match event {
            Event::Start(s) => stack.push(element(&s)?),
            Event::Empty(s) => {
                let e = element(&s)?;
                push(&mut stack, &mut top, Node::Element(e));
            }
            Event::End(_) => {
                let e = stack.pop().context("unbalanced XML end tag")?;
                push(&mut stack, &mut top, Node::Element(e));
            }
            Event::Text(t) => {
                let text = t.unescape().context("malformed XML text")?.into_owned();
                push(&mut stack, &mut top, Node::Text(text));
            }
            Event::CData(c) => {
                let raw = format!("<![CDATA[{}]]>", utf8(&c)?);
                push(&mut stack, &mut top, Node::Raw(raw));
            }
            Event::Comment(c) => {
                let raw = format!("<!--{}-->", utf8(&c)?);
                push(&mut stack, &mut top, Node::Raw(raw));
            }
            Event::Decl(d) => {
                let raw = format!("<?{}?>", utf8(&d)?);
                push(&mut stack, &mut top, Node::Raw(raw));
            }
            Event::PI(p) => {
                let raw = format!("<?{}?>", utf8(&p)?);
                push(&mut stack, &mut top, Node::Raw(raw));
            }
            Event::DocType(d) => {
                let raw = format!("<!DOCTYPE {}>", utf8(&d)?);
                push(&mut stack, &mut top, Node::Raw(raw));
            }
            Event::Eof => break,
        }
    }
    anyhow::ensure!(stack.is_empty(), "XML ended inside <{}>", stack.last().map_or("", |e| e.name.as_str()));
    Ok(top)
}

fn escape(s: &str, attr: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\n' if attr => out.push_str("&#xA;"),
            _ => out.push(c),
        }
    }
    out
}

fn write_node(node: &Node, out: &mut String) {
    match node {
        Node::Text(t) => out.push_str(&escape(t, false)),
        Node::Raw(r) => out.push_str(r),
        Node::Element(e) => {
            out.push('<');
            out.push_str(&e.name);
            for (name, value) in &e.attrs {
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                out.push_str(&escape(value, true));
                out.push('"');
            }
            if e.children.is_empty() {
                out.push_str("/>");
            } else {
                out.push('>');
                for child in &e.children {
                    write_node(child, out);
                }
                out.push_str("</");
                out.push_str(&e.name);
                out.push('>');
            }
        }
    }
}

pub(crate) fn write(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        write_node(n, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_verbatim_enough() {
        let xml = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
                   <x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><!-- note -->\n \
                   <a:b a:c=\"1 &amp; 2\">t &lt; u</a:b><e/>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>";
        let nodes = parse(xml).unwrap();
        assert_eq!(write(&nodes), xml);
    }

    #[test]
    fn rejects_broken_xml() {
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("<a>").is_err());
    }
}
