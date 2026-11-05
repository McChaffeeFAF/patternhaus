//! SFZ text: encoding, comments, the preprocessor (`#define`, `#include`) and tokens
//! (`docs/formats/sfz.md` §1–2).

use crate::notes::Notes;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Include depth cap (§2.2).
pub const MAX_INCLUDE_DEPTH: usize = 32;

/// Opcodes whose value runs to the end of the line or the next `name=` (§1.3).
const SPACE_VALUES: &[&str] = &[
    "sample",
    "default_path",
    "sw_label",
    "global_label",
    "master_label",
    "group_label",
    "region_label",
    "image",
    "tdfir_impulse",
];

fn space_value(name: &str) -> bool {
    SPACE_VALUES.contains(&name)
        || ["label_cc", "label_key", "label_output"]
            .iter()
            .any(|p| name.strip_prefix(p).is_some_and(is_digits))
}

pub fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// One token of the stream, after the preprocessor.
#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Header(String),
    Opcode { name: String, value: String },
}

/// Text as UTF-8, or Latin-1 for the whole file when it is not valid UTF-8 (§1.1).
pub fn decode_text(bytes: &[u8], file: &str, notes: &mut Notes) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            notes.approx(
                "text:latin-1",
                format!("{file} is not valid UTF-8; read as Latin-1"),
            );
            bytes.iter().map(|&b| b as char).collect()
        }
    }
}

/// Line endings to LF, `//` line comments and `/* */` block comments removed (newlines inside
/// a block comment are kept so lines stay lines).
pub fn strip_comments(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'/') {
            for d in chars.by_ref() {
                if d == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prev = '\0';
            for d in chars.by_ref() {
                if d == '\n' {
                    out.push('\n');
                }
                if prev == '*' && d == '/' {
                    break;
                }
                prev = d;
            }
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Where the preprocessor reads `#include`d files from.
pub struct Includes<'a> {
    /// Folder of the main file: include paths resolve against it and must stay inside it.
    pub root: Option<&'a Path>,
}

/// A preprocessed item before `#define` substitution.
#[derive(Debug)]
enum Item {
    Text(String),
    Define(String, String),
}

/// Lexically normalised `rel` under `root`, or `None` if it escapes `root` or is absolute.
pub fn confined(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.replace('\\', "/");
    if rel.starts_with('/') || rel.contains(':') {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for c in Path::new(&rel).components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    let mut p = root.to_path_buf();
    for s in parts {
        p.push(s);
    }
    Some(p)
}

fn expand(
    text: &str,
    inc: &Includes<'_>,
    chain: &mut Vec<PathBuf>,
    out: &mut Vec<Item>,
    notes: &mut Notes,
) {
    for line in strip_comments(text).split('\n') {
        let mut rest = line;
        loop {
            let pos = [rest.find("#define"), rest.find("#include")]
                .into_iter()
                .flatten()
                .min();
            let Some(pos) = pos else {
                out.push(Item::Text(rest.to_string()));
                break;
            };
            out.push(Item::Text(rest[..pos].to_string()));
            let after = &rest[pos..];
            if let Some(a) = after.strip_prefix("#define") {
                let a = a.trim_start();
                let name_end = a.find(char::is_whitespace).unwrap_or(a.len());
                let name = &a[..name_end];
                let b = a[name_end..].trim_start();
                let val_end = b.find(char::is_whitespace).unwrap_or(b.len());
                let value = &b[..val_end];
                if name.len() > 1
                    && name.starts_with('$')
                    && name[1..]
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                {
                    out.push(Item::Define(name.to_string(), value.to_string()));
                } else {
                    notes.skip(
                        "directive:#define",
                        "a #define without a valid $NAME was ignored",
                    );
                }
                rest = &b[val_end..];
            } else {
                let a = after["#include".len()..].trim_start();
                let Some(q) = a.strip_prefix('"') else {
                    notes.skip(
                        "directive:#include",
                        "an #include without a quoted path was ignored",
                    );
                    rest = a;
                    continue;
                };
                let Some(end) = q.find('"') else {
                    notes.skip(
                        "directive:#include",
                        "an #include without a closing quote was ignored",
                    );
                    break;
                };
                include(&q[..end], inc, chain, out, notes);
                rest = &q[end + 1..];
            }
        }
    }
}

fn include(
    rel: &str,
    inc: &Includes<'_>,
    chain: &mut Vec<PathBuf>,
    out: &mut Vec<Item>,
    notes: &mut Notes,
) {
    let what = format!("include:{rel}");
    let Some(root) = inc.root else {
        notes.skip(what, "no folder to read included files from");
        return;
    };
    let Some(path) = confined(root, rel) else {
        notes.skip(
            what,
            "the include path leaves the instrument's folder; refused",
        );
        return;
    };
    if chain.contains(&path) {
        let names: Vec<String> = chain
            .iter()
            .chain(std::iter::once(&path))
            .map(|p| p.display().to_string())
            .collect();
        notes.skip(what, format!("include cycle: {}", names.join(" -> ")));
        return;
    }
    if chain.len() >= MAX_INCLUDE_DEPTH {
        notes.skip(
            what,
            format!("includes nest deeper than {MAX_INCLUDE_DEPTH}"),
        );
        return;
    }
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            notes.skip(what, format!("cannot read the included file: {e}"));
            return;
        }
    };
    let text = decode_text(&bytes, rel, notes);
    chain.push(path);
    expand(&text, inc, chain, out, notes);
    chain.pop();
}

/// Replaces `$NAME`s with their values, the longest defined name first (§2.1).
fn substitute(text: &str, defines: &BTreeMap<String, String>, notes: &mut Notes) -> String {
    if !text.contains('$') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(off) = text[i..].find('$') {
        out.push_str(&text[i..i + off]);
        let at = i + off;
        let tail = &text[at..];
        let run = 1 + tail[1..]
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
            .count();
        // longest defined prefix of the run
        let hit = (2..=run)
            .rev()
            .find_map(|n| defines.get(&tail[..n]).map(|v| (n, v)));
        match hit {
            Some((n, v)) => {
                out.push_str(v);
                i = at + n;
            }
            None => {
                if run > 1 {
                    notes.skip(
                        format!("variable:{}", &tail[..run]),
                        "used but never #defined; left as text",
                    );
                }
                out.push('$');
                i = at + 1;
            }
        }
    }
    out.push_str(&text[i..]);
    out
}

fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// At `s[j..]` (just after whitespace): a run of name bytes immediately followed by `=`.
fn next_opcode_at(s: &[u8], j: usize) -> bool {
    let n = s[j..].iter().take_while(|c| is_name_byte(**c)).count();
    n > 0 && s.get(j + n) == Some(&b'=')
}

fn tokenize(seg: &str, out: &mut Vec<Tok>, notes: &mut Notes) {
    let s = seg.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if s[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if s[i] == b'<' {
            match seg[i..].find('>') {
                Some(e) => {
                    out.push(Tok::Header(seg[i + 1..i + e].trim().to_string()));
                    i += e + 1;
                }
                None => {
                    notes.unknown(format!("text:{}", seg[i..].trim()));
                    return;
                }
            }
            continue;
        }
        let start = i;
        while i < s.len() && !s[i].is_ascii_whitespace() && s[i] != b'=' && s[i] != b'<' {
            i += 1;
        }
        let name = &seg[start..i];
        let mut j = i;
        while j < s.len() && (s[j] == b' ' || s[j] == b'\t') {
            j += 1;
        }
        if j >= s.len() || s[j] != b'=' || name.is_empty() {
            notes.unknown(format!("text:{}", if name.is_empty() { "=" } else { name }));
            i = i.max(start + 1);
            continue;
        }
        j += 1;
        while j < s.len() && (s[j] == b' ' || s[j] == b'\t') {
            j += 1;
        }
        let vstart = j;
        if space_value(name) {
            let mut k = j;
            while k < s.len() {
                if s[k] == b'<' {
                    break;
                }
                if s[k].is_ascii_whitespace() {
                    let mut w = k;
                    while w < s.len() && s[w].is_ascii_whitespace() {
                        w += 1;
                    }
                    if w < s.len() && (next_opcode_at(s, w) || s[w] == b'<') {
                        break;
                    }
                    k = w;
                    continue;
                }
                k += 1;
            }
            j = k;
        } else {
            while j < s.len() && !s[j].is_ascii_whitespace() && s[j] != b'<' {
                j += 1;
            }
        }
        out.push(Tok::Opcode {
            name: name.to_string(),
            value: seg[vstart..j].trim().to_string(),
        });
        i = j;
    }
}

/// The whole token stream of a main file: comments removed, includes expanded, defines
/// substituted in stream order, then tokens (§2.3).
pub fn tokens(
    text: &str,
    inc: &Includes<'_>,
    main: Option<PathBuf>,
    notes: &mut Notes,
) -> Vec<Tok> {
    let mut items = Vec::new();
    let mut chain: Vec<PathBuf> = main.into_iter().collect();
    expand(text, inc, &mut chain, &mut items, notes);
    let mut defines = BTreeMap::new();
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::Define(n, v) => {
                defines.insert(n, v);
            }
            Item::Text(t) => {
                if !t.trim().is_empty() {
                    let t = substitute(&t, &defines, notes);
                    tokenize(&t, &mut out, notes);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(text: &str) -> (Vec<Tok>, Notes) {
        let mut n = Notes::default();
        let t = tokens(text, &Includes { root: None }, None, &mut n);
        (t, n)
    }

    fn op(name: &str, value: &str) -> Tok {
        Tok::Opcode {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn comments_line_endings_and_spaced_values() {
        let (t, _) = toks(
            "<region> sample=My Piano\\C 4.wav lokey=60 // note\r\n/* block\n comment */<group>\tvolume = -3\rhikey=c#4",
        );
        assert_eq!(
            t,
            vec![
                Tok::Header("region".into()),
                op("sample", "My Piano\\C 4.wav"),
                op("lokey", "60"),
                Tok::Header("group".into()),
                op("volume", "-3"),
                op("hikey", "c#4"),
            ]
        );
    }

    #[test]
    fn defines_longest_first_and_in_stream_order() {
        let (t, n) = toks(
            "#define $VEL 10\n#define $VELHI 20\n<region> lovel=$VEL hivel=$VELHI amplitude_oncc$VEL=5 x=$NOPE\n#define $VEL 30\nlovel=$VEL",
        );
        assert_eq!(
            t,
            vec![
                Tok::Header("region".into()),
                op("lovel", "10"),
                op("hivel", "20"),
                op("amplitude_oncc10", "5"),
                op("x", "$NOPE"),
                op("lovel", "30"),
            ]
        );
        assert!(n.has("variable:$NOPE"));
    }

    #[test]
    fn includes_are_confined_and_cycles_stop() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(
            dir.path().join("sub/a.sfzh"),
            "#define $K 61\n<region> key=$K\n#include \"sub/b.sfzh\"",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("sub/b.sfzh"),
            "hivel=99 #include \"sub/a.sfzh\"",
        )
        .unwrap();
        let mut n = Notes::default();
        let main = dir.path().join("main.sfz");
        let t = tokens(
            "#include \"sub/a.sfzh\" lovel=$K\n#include \"../x.sfz\"\n#include \"missing.sfzh\"",
            &Includes {
                root: Some(dir.path()),
            },
            Some(main),
            &mut n,
        );
        assert_eq!(
            t,
            vec![
                Tok::Header("region".into()),
                op("key", "61"),
                op("hivel", "99"),
                op("lovel", "61"),
            ]
        );
        assert!(n.has("include:sub/a.sfzh"), "cycle reported");
        assert!(n.has("include:../x.sfz"), "escape refused");
        assert!(n.has("include:missing.sfzh"));
    }

    #[test]
    fn junk_is_reported_not_fatal() {
        let (t, n) = toks("lokey 60 <region> <group");
        assert_eq!(t, vec![Tok::Header("region".into())]);
        let (a, b) = n.finish();
        assert!(!a.is_empty() || !b.is_empty());
    }
}
