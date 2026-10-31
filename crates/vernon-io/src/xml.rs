//! A small pull tokenizer for the XML found in 3MF / OPC packages.
//!
//! Emits start and end events only (text is skipped, an empty element yields
//! both). Declarations, comments, CDATA and DOCTYPE are skipped; only the five
//! predefined entities and numeric character references are decoded, so no
//! entity expansion attack is possible. Element names are reported without
//! their namespace prefix; attributes keep theirs.
//!
//! The text that sat in front of the most recent event is available from
//! [`Reader::text`]; after an end tag that is the element's character data.
//! Markup inside that run (a comment, say) stays in it as raw text, so a
//! numeric parse of such a value fails instead of silently reading half of it.

use std::borrow::Cow;

/// Deepest element nesting accepted.
const MAX_DEPTH: usize = 128;

#[derive(Debug)]
pub(crate) struct Tag<'a> {
    /// Local name (prefix stripped).
    pub name: &'a str,
    pub attrs: Vec<(&'a str, Cow<'a, str>)>,
}

impl<'a> Tag<'a> {
    /// Value of the attribute with exactly this (qualified) name.
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_ref())
    }

    /// Value of the attribute whose local name matches, ignoring any prefix.
    pub(crate) fn attr_local(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| local(k) == name)
            .map(|(_, v)| v.as_ref())
    }
}

#[derive(Debug)]
pub(crate) enum Event<'a> {
    Start(Tag<'a>),
    End(&'a str),
}

fn local(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

pub(crate) struct Reader<'a> {
    src: &'a str,
    pos: usize,
    stack: Vec<&'a str>,
    pending_end: Option<&'a str>,
    /// Raw text between the previous event and the one just returned.
    last_text: &'a str,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        Self {
            src,
            pos: 0,
            stack: Vec::new(),
            pending_end: None,
            last_text: "",
        }
    }

    /// Character data in front of the most recent event, trimmed and with the
    /// predefined entities decoded. Empty for an element written as `<a/>`.
    pub(crate) fn text(&self) -> Result<Cow<'a, str>, String> {
        unescape(self.last_text.trim())
    }

    fn err<T>(&self, at: usize, msg: &str) -> Result<T, String> {
        let line = self.src.as_bytes()[..at.min(self.src.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1;
        Err(format!("XML error at line {line}: {msg}"))
    }

    /// Next event, `Ok(None)` at a clean end of document.
    pub(crate) fn next_event(&mut self) -> Result<Option<Event<'a>>, String> {
        if let Some(n) = self.pending_end.take() {
            self.last_text = "";
            return Ok(Some(Event::End(n)));
        }
        let run_start = self.pos;
        loop {
            let Some(rel) = self.src[self.pos..].find('<') else {
                self.pos = self.src.len();
                return match self.stack.last() {
                    None => Ok(None),
                    Some(open) => self.err(self.pos, &format!("element <{open}> is never closed")),
                };
            };
            let at = self.pos + rel;
            let rest = &self.src[at..];
            if rest.starts_with("<?") {
                self.pos = self.skip_to(at, "?>")?;
            } else if rest.starts_with("<!--") {
                self.pos = self.skip_to(at + 4, "-->")?;
            } else if rest.starts_with("<![CDATA[") {
                self.pos = self.skip_to(at + 9, "]]>")?;
            } else if rest.starts_with("<!") {
                self.pos = self.skip_doctype(at)?;
            } else if rest.starts_with("</") {
                let Some(close) = rest.find('>') else {
                    return self.err(at, "unterminated end tag");
                };
                let name = rest[2..close].trim();
                self.last_text = &self.src[run_start..at];
                self.pos = at + close + 1;
                return match self.stack.pop() {
                    Some(open) if open == name => Ok(Some(Event::End(local(open)))),
                    Some(open) => self.err(at, &format!("</{name}> closes <{open}>")),
                    None => self.err(at, &format!("stray </{name}>")),
                };
            } else {
                self.last_text = &self.src[run_start..at];
                return self.start_tag(at).map(Some);
            }
        }
    }

    fn skip_to(&self, from: usize, pat: &str) -> Result<usize, String> {
        match self.src[from..].find(pat) {
            Some(i) => Ok(from + i + pat.len()),
            None => self.err(from, &format!("missing '{pat}'")),
        }
    }

    /// Skip `<!DOCTYPE ... [ ... ]>` without interpreting its contents.
    fn skip_doctype(&self, at: usize) -> Result<usize, String> {
        let mut depth = 0i32;
        let mut quote = 0u8;
        for (i, &b) in self.src.as_bytes()[at..].iter().enumerate() {
            match b {
                b'"' | b'\'' if quote == 0 => quote = b,
                q if q == quote => quote = 0,
                b'[' if quote == 0 => depth += 1,
                b']' if quote == 0 => depth -= 1,
                b'>' if quote == 0 && depth <= 0 => return Ok(at + i + 1),
                _ => {}
            }
        }
        self.err(at, "unterminated declaration")
    }

    fn start_tag(&mut self, at: usize) -> Result<Event<'a>, String> {
        let src = self.src;
        let b = src.as_bytes();
        let mut i = at + 1;
        let name_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'/' && b[i] != b'>' {
            i += 1;
        }
        let qname = &src[name_start..i];
        if qname.is_empty() {
            return self.err(at, "element without a name");
        }
        let mut attrs: Vec<(&'a str, Cow<'a, str>)> = Vec::new();
        let empty;
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            match b.get(i) {
                None => return self.err(at, "unterminated start tag"),
                Some(b'>') => {
                    empty = false;
                    i += 1;
                    break;
                }
                Some(b'/') => {
                    if b.get(i + 1) != Some(&b'>') {
                        return self.err(i, "stray '/' in start tag");
                    }
                    empty = true;
                    i += 2;
                    break;
                }
                Some(_) => {}
            }
            let ks = i;
            while i < b.len()
                && !b[i].is_ascii_whitespace()
                && b[i] != b'='
                && b[i] != b'>'
                && b[i] != b'/'
            {
                i += 1;
            }
            let key = &src[ks..i];
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if key.is_empty() || b.get(i) != Some(&b'=') {
                return self.err(ks, "attribute without '='");
            }
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let q = match b.get(i) {
                Some(&q) if q == b'"' || q == b'\'' => q,
                _ => return self.err(i, "attribute value is not quoted"),
            };
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            if i >= b.len() {
                return self.err(vs, "unterminated attribute value");
            }
            let value = match unescape(&src[vs..i]) {
                Ok(v) => v,
                Err(m) => return self.err(vs, &m),
            };
            i += 1;
            if !attrs.iter().any(|(k, _)| *k == key) {
                attrs.push((key, value));
            }
        }
        self.pos = i;
        if empty {
            self.pending_end = Some(local(qname));
        } else {
            if self.stack.len() >= MAX_DEPTH {
                return self.err(at, "elements nested too deeply");
            }
            self.stack.push(qname);
        }
        Ok(Event::Start(Tag {
            name: local(qname),
            attrs,
        }))
    }
}

/// Decode the predefined entities and numeric references.
fn unescape(s: &str) -> Result<Cow<'_, str>, String> {
    if !s.contains('&') {
        return Ok(Cow::Borrowed(s));
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let Some(end) = rest.find(';') else {
            return Err("unterminated entity".into());
        };
        let ent = &rest[..end];
        match ent {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code =
                    if let Some(h) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                        u32::from_str_radix(h, 16).ok()
                    } else if let Some(d) = ent.strip_prefix('#') {
                        d.parse::<u32>().ok()
                    } else {
                        None
                    };
                match code.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => return Err(format!("unknown entity '&{ent};'")),
                }
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(Cow::Owned(out))
}

/// Escape text for use inside a double-quoted attribute value.
pub(crate) fn escape_attr(s: &str) -> Cow<'_, str> {
    if !s.contains(['&', '<', '>', '"', '\'']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(src: &str) -> Result<Vec<String>, String> {
        let mut r = Reader::new(src);
        let mut out = Vec::new();
        while let Some(e) = r.next_event()? {
            out.push(match e {
                Event::Start(t) => {
                    let a: Vec<String> = t.attrs.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    format!("<{} {}>", t.name, a.join(","))
                }
                Event::End(n) => format!("</{n}>"),
            });
        }
        Ok(out)
    }

    #[test]
    fn parses_nesting_empty_elements_and_prefixes() {
        let src = "\u{feff}<?xml version=\"1.0\"?><!-- c --><m:model unit='inch' x:y=\"1\">\n\
                   <a id=\"1\"/><b>text &amp; <![CDATA[<raw>]]></b></m:model>";
        let ev = events(src).unwrap();
        assert_eq!(
            ev,
            [
                "<model unit=inch,x:y=1>",
                "<a id=1>",
                "</a>",
                "<b >",
                "</b>",
                "</model>"
            ]
        );
        let mut r = Reader::new("<a p:path=\"/x\" b=\"2\"/>");
        let Some(Event::Start(t)) = r.next_event().unwrap() else {
            panic!()
        };
        assert_eq!(t.attr("b"), Some("2"));
        assert_eq!(t.attr("path"), None);
        assert_eq!(t.attr_local("path"), Some("/x"));
    }

    #[test]
    fn decodes_entities() {
        let ev = events("<a v=\"&lt;&gt;&amp;&quot;&apos;&#65;&#x42;\"/>").unwrap();
        assert_eq!(ev[0], "<a v=<>&\"'AB>");
        assert!(events("<a v=\"&bogus;\"/>").is_err());
        assert!(events("<a v=\"&amp\"/>").is_err());
        assert_eq!(escape_attr("a<b&\"c\""), "a&lt;b&amp;&quot;c&quot;");
        assert_eq!(escape_attr("plain"), "plain");
    }

    #[test]
    fn doctype_is_skipped_and_never_expanded() {
        let src = "<!DOCTYPE m [<!ENTITY a \"&b;&b;\"><!ENTITY b \"x\">]><m v=\"1\"/>";
        assert_eq!(events(src).unwrap(), ["<m v=1>", "</m>"]);
        assert!(events("<m v=\"&a;\"/>").is_err());
    }

    #[test]
    fn malformed_documents_are_errors() {
        for bad in [
            "<a>",
            "<a></b>",
            "</a>",
            "<a",
            "<a b>",
            "<a b=1/>",
            "<a b=\"1/>",
            "<a / >",
            "<!-- never ends",
            "<a><![CDATA[x</a>",
            "<>",
        ] {
            assert!(events(bad).is_err(), "{bad:?}");
        }
        let deep = "<a>".repeat(MAX_DEPTH + 1);
        assert!(events(&deep).is_err());
    }

    #[test]
    fn error_reports_line() {
        let e = events("<a>\n\n</b>").unwrap_err();
        assert!(e.contains("line 3"), "{e}");
    }

    #[test]
    fn every_truncation_is_handled() {
        let src = "<?xml version=\"1.0\"?><model unit=\"mm\"><vertices><vertex x=\"1\" y=\"2\" z=\"3\"/></vertices></model>";
        for n in 0..src.len() {
            let _ = events(&src[..n]);
        }
    }
}
