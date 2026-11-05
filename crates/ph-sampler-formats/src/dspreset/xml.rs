//! A small XML 1.0 reader of our own (`docs/formats/m3-dependencies.md`, option A):
//! elements, attributes, text, comments, CDATA, processing instructions, the five predefined
//! entities and numeric character references. A DOCTYPE is skipped (no DTD, no external
//! entities). Documents that are not well-formed (mismatched or unclosed tags, duplicate
//! attributes, unknown entities, junk after the root) are refused with a line number.
//! [`Element::to_xml`] writes a subtree back out (the `<ui>` kept as data).

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Element(Element),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    /// 1-based line of the start tag.
    pub line: usize,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Child elements in document order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.elements().filter(move |e| e.name == name)
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }

    /// The subtree as XML text.
    pub fn to_xml(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    fn write(&self, s: &mut String) {
        s.push('<');
        s.push_str(&self.name);
        for (k, v) in &self.attrs {
            s.push(' ');
            s.push_str(k);
            s.push_str("=\"");
            s.push_str(&escape(v));
            s.push('"');
        }
        if self.children.is_empty() {
            s.push_str("/>");
            return;
        }
        s.push('>');
        for c in &self.children {
            match c {
                Node::Element(e) => e.write(s),
                Node::Text(t) => s.push_str(&escape(t)),
            }
        }
        s.push_str("</");
        s.push_str(&self.name);
        s.push('>');
    }
}

fn escape(t: &str) -> String {
    let mut o = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

/// Deepest element nesting accepted (keeps damaged input from exhausting the stack).
const MAX_DEPTH: usize = 200;

struct P<'a> {
    s: &'a str,
    i: usize,
    /// (byte offset, line there): lines are counted forward from the last query.
    at: std::cell::Cell<(usize, usize)>,
}

type R<T> = Result<T, String>;

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == ':'
}

fn is_name(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.' || c.is_alphanumeric()
}

impl<'a> P<'a> {
    fn line(&self) -> usize {
        let (from, line) = self.at.get();
        let (from, line) = if from <= self.i { (from, line) } else { (0, 1) };
        let line = line + self.s[from..self.i].matches('\n').count();
        self.at.set((self.i, line));
        line
    }
    fn err<T>(&self, what: &str) -> R<T> {
        Err(format!("line {}: {what}", self.line()))
    }
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn starts(&self, t: &str) -> bool {
        self.rest().starts_with(t)
    }
    fn bump(&mut self, n: usize) {
        self.i += n;
    }
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.bump(c.len_utf8());
            } else {
                break;
            }
        }
    }
    /// Skips past `end`, or fails with `what`.
    fn skip_past(&mut self, end: &str, what: &str) -> R<()> {
        match self.rest().find(end) {
            Some(k) => {
                self.bump(k + end.len());
                Ok(())
            }
            None => self.err(what),
        }
    }
    fn name(&mut self) -> R<String> {
        let start = self.i;
        match self.peek() {
            Some(c) if is_name_start(c) => self.bump(c.len_utf8()),
            _ => return self.err("expected a name"),
        }
        while let Some(c) = self.peek() {
            if is_name(c) {
                self.bump(c.len_utf8());
            } else {
                break;
            }
        }
        Ok(self.s[start..self.i].to_string())
    }
    /// Comments, processing instructions and the DOCTYPE between nodes.
    fn misc(&mut self) -> R<bool> {
        if self.starts("<!--") {
            self.skip_past("-->", "unclosed comment")?;
            Ok(true)
        } else if self.starts("<?") {
            self.skip_past("?>", "unclosed processing instruction")?;
            Ok(true)
        } else if self.starts("<!DOCTYPE") {
            // skip to the matching '>' (an internal subset in brackets may hold '>')
            let mut depth = 0i32;
            for (k, c) in self.rest().char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    '>' if depth <= 0 => {
                        self.bump(k + 1);
                        return Ok(true);
                    }
                    _ => {}
                }
            }
            self.err("unclosed DOCTYPE")
        } else {
            Ok(false)
        }
    }
    fn text(&mut self, raw: &str) -> R<String> {
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(k) = rest.find('&') {
            out.push_str(&rest[..k]);
            rest = &rest[k + 1..];
            let Some(end) = rest.find(';') else {
                return self.err("an '&' that starts no entity");
            };
            let ent = &rest[..end];
            rest = &rest[end + 1..];
            let c = match ent {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| {
                            format!("line {}: bad character reference &{ent};", self.line())
                        })?
                }
                _ if ent.starts_with('#') => ent[1..]
                    .parse::<u32>()
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| {
                        format!("line {}: bad character reference &{ent};", self.line())
                    })?,
                _ => return self.err(&format!("unknown entity &{ent};")),
            };
            out.push(c);
        }
        out.push_str(rest);
        Ok(out)
    }
    fn element(&mut self, depth: usize) -> R<Element> {
        if depth > MAX_DEPTH {
            return self.err("elements nested too deeply");
        }
        let line = self.line();
        self.bump(1); // '<'
        let name = self.name()?;
        let mut e = Element {
            name,
            line,
            ..Default::default()
        };
        loop {
            let before = self.i;
            self.skip_ws();
            if self.starts("/>") {
                self.bump(2);
                return Ok(e);
            }
            if self.starts(">") {
                self.bump(1);
                break;
            }
            if self.i == before && !e.attrs.is_empty() {
                return self.err("attributes must be separated by white space");
            }
            let k = self.name()?;
            self.skip_ws();
            if !self.starts("=") {
                return self.err(&format!("attribute {k} has no value"));
            }
            self.bump(1);
            self.skip_ws();
            let q = match self.peek() {
                Some(q @ ('"' | '\'')) => q,
                _ => return self.err(&format!("attribute {k}'s value is not quoted")),
            };
            self.bump(1);
            let Some(end) = self.rest().find(q) else {
                return self.err(&format!("attribute {k}'s value is not closed"));
            };
            let raw = &self.rest()[..end];
            if raw.contains('<') {
                return self.err(&format!("'<' in attribute {k}"));
            }
            let v = self.text(raw)?;
            self.bump(end + 1);
            if e.attrs.iter().any(|(n, _)| *n == k) {
                return self.err(&format!("attribute {k} appears twice"));
            }
            e.attrs.push((k, v));
        }
        // content
        loop {
            if self.i >= self.s.len() {
                return self.err(&format!("<{}> is never closed", e.name));
            }
            if self.starts("</") {
                self.bump(2);
                let n = self.name()?;
                self.skip_ws();
                if !self.starts(">") {
                    return self.err("a closing tag is not closed");
                }
                self.bump(1);
                if n != e.name {
                    return self.err(&format!("</{n}> closes <{}>", e.name));
                }
                return Ok(e);
            }
            if self.starts("<![CDATA[") {
                self.bump(9);
                let Some(end) = self.rest().find("]]>") else {
                    return self.err("unclosed CDATA section");
                };
                let t = self.rest()[..end].to_string();
                self.bump(end + 3);
                e.children.push(Node::Text(t));
                continue;
            }
            if self.misc()? {
                continue;
            }
            if self.starts("<") {
                let child = self.element(depth + 1)?;
                e.children.push(Node::Element(child));
                continue;
            }
            let end = self.rest().find('<').unwrap_or(self.rest().len());
            let raw = &self.rest()[..end];
            let t = self.text(raw)?;
            self.bump(end);
            if !t.trim().is_empty() {
                e.children.push(Node::Text(t));
            }
        }
    }
}

/// Parses a document; returns its root element.
pub fn parse(text: &str) -> Result<Element, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = P {
        s: text,
        i: 0,
        at: std::cell::Cell::new((0, 1)),
    };
    loop {
        p.skip_ws();
        if !p.misc()? {
            break;
        }
    }
    if !p.starts("<") {
        return p.err("no root element");
    }
    let root = p.element(0)?;
    loop {
        p.skip_ws();
        if p.i >= p.s.len() {
            return Ok(root);
        }
        if !p.misc()? {
            return p.err("content after the root element");
        }
    }
}

/// Bytes to text: UTF-8 (BOM allowed), else Latin-1.
pub fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_elements_attributes_text_and_entities() {
        let doc = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE x [<!ENTITY a \"b\">]>\n\
                   <!-- c --><a x='1' y=\"&lt;&amp;&#65;&#x42;\">\n  <b/> t&gt; <![CDATA[<raw>]]><?pi?><c z=\"2\"></c>\n</a><!-- end -->\n";
        let r = parse(doc).unwrap();
        assert_eq!(r.name, "a");
        assert_eq!(r.attr("y"), Some("<&AB"));
        assert_eq!(
            r.elements().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert_eq!(r.child("c").unwrap().line, 4);
        assert!(r.children.contains(&Node::Text("<raw>".into())));
        let back = parse(&r.to_xml()).unwrap();
        assert_eq!(back.attr("y"), Some("<&AB"));
        assert_eq!(back.child("c").unwrap().attr("z"), Some("2"));
    }

    #[test]
    fn refuses_documents_that_are_not_well_formed() {
        for bad in [
            "",
            "text",
            "<a>",
            "<a></b>",
            "<a x=1/>",
            "<a x='1' x='2'/>",
            "<a>&nope;</a>",
            "<a/><b/>",
            "<a x='<'/>",
            "<a><!-- </a>",
            "<a x='1'y='2'/>",
            "<a>&#xFFFFFFFF;</a>",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = "<a>".repeat(500);
        assert!(parse(&deep).is_err());
    }
}
