//! Browser data: factory kits, Haus Synth presets, project and library files.
//!
//! Pure scanning code (no egui), so the tree and the sample references it produces are tested
//! against a temp folder.

use ph_core::InstrumentSpec;
use std::path::{Path, PathBuf};

const AUDIO_EXT: &[&str] = &["wav", "flac", "aif", "aiff", "mp3", "ogg"];

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Folder,
    /// A sample file with the reference the project stores (`factory:...`, relative or absolute).
    Sample {
        reference: String,
    },
    /// A Haus Synth preset (`category/name`).
    Preset {
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub kind: Kind,
    pub children: Vec<Node>,
}

impl Node {
    pub fn is_folder(&self) -> bool {
        self.kind == Kind::Folder
    }

    /// Every sample reference under this node (for "add kit").
    pub fn samples(&self) -> Vec<(&str, &str)> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<(&'a str, &'a str)>) {
        if let Kind::Sample { reference } = &self.kind {
            out.push((self.name.as_str(), reference.as_str()));
        }
        for c in &self.children {
            c.collect(out);
        }
    }
}

/// How a sample's reference is written.
#[derive(Clone, Debug)]
pub enum RefStyle {
    /// `factory:<path relative to the samples folder, no extension>`
    Factory,
    /// Path relative to a root, forward slashes (project files).
    RelativeTo(PathBuf),
    /// Absolute path (library folders).
    Absolute,
}

fn is_audio(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

fn reference(path: &Path, root: &Path, style: &RefStyle) -> String {
    let rel = |base: &Path| {
        path.strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    match style {
        RefStyle::Factory => {
            let r = rel(root);
            format!(
                "factory:{}",
                r.rsplit_once('.').map_or(r.as_str(), |(a, _)| a)
            )
        }
        RefStyle::RelativeTo(base) => rel(base),
        RefStyle::Absolute => path.to_string_lossy().replace('\\', "/"),
    }
}

/// Scans `root` for audio files, folders first, names sorted. Empty folders are dropped.
pub fn scan(root: &Path, style: &RefStyle, max_depth: usize) -> Vec<Node> {
    scan_dir(root, root, style, max_depth)
}

fn scan_dir(dir: &Path, root: &Path, style: &RefStyle, depth: usize) -> Vec<Node> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort_by_key(|p| (!p.is_dir(), p.file_name().map(|n| n.to_ascii_lowercase())));
    let mut out = Vec::new();
    for p in entries {
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if p.is_dir() {
            if depth == 0
                || p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let children = scan_dir(&p, root, style, depth - 1);
            if !children.is_empty() {
                out.push(Node {
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    kind: Kind::Folder,
                    children,
                });
            }
        } else if is_audio(&p) {
            out.push(Node {
                name,
                kind: Kind::Sample {
                    reference: reference(&p, root, style),
                },
                children: Vec::new(),
            });
        }
    }
    out
}

/// Factory samples under `<assets>/samples`.
pub fn factory_samples(assets_dir: &Path) -> Vec<Node> {
    scan(&assets_dir.join("samples"), &RefStyle::Factory, 6)
}

/// Presets grouped by category (`bass/sub` -> folder `bass`, preset `sub`).
pub fn synth_presets(names: &[&str]) -> Vec<Node> {
    let mut cats: Vec<Node> = Vec::new();
    for full in names {
        let (cat, leaf) = full.split_once('/').unwrap_or(("other", full));
        let node = Node {
            name: leaf.to_string(),
            kind: Kind::Preset {
                name: (*full).to_string(),
            },
            children: Vec::new(),
        };
        match cats.iter_mut().find(|c| c.name == cat) {
            Some(c) => c.children.push(node),
            None => cats.push(Node {
                name: cat.to_string(),
                kind: Kind::Folder,
                children: vec![node],
            }),
        }
    }
    cats.sort_by(|a, b| a.name.cmp(&b.name));
    for c in &mut cats {
        c.children.sort_by(|a, b| a.name.cmp(&b.name));
    }
    cats
}

/// The instrument a dragged or double-clicked browser entry creates.
pub fn instrument_for(kind: &Kind) -> Option<InstrumentSpec> {
    match kind {
        Kind::Sample { reference } => Some(InstrumentSpec::Sampler {
            slices: Vec::new(),
            sample: Some(reference.clone()),
            root_key: 60,
            params: Default::default(),
        }),
        Kind::Preset { name } => Some(InstrumentSpec::Synth {
            preset: Some(name.clone()),
            params: Default::default(),
        }),
        Kind::Folder => None,
    }
}

/// Channel name for a browser entry: "kick", "hat closed" -> "Kick", "Hat closed".
pub fn channel_name(node_name: &str) -> String {
    let s = node_name.replace(['_', '-'], " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// Payload carried by a browser drag.
#[derive(Clone, Debug, PartialEq)]
pub struct Dragged {
    pub name: String,
    pub kind: Kind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_audio_files_into_a_tree_with_references() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("samples");
        std::fs::create_dir_all(root.join("drums/kit")).unwrap();
        std::fs::write(root.join("drums/kit/kick.wav"), b"x").unwrap();
        std::fs::write(root.join("drums/kit/notes.txt"), b"x").unwrap();
        std::fs::write(root.join("drums/snare.flac"), b"x").unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        let tree = scan(&root, &RefStyle::Factory, 6);
        assert_eq!(tree.len(), 1, "empty folders are dropped");
        let drums = &tree[0];
        assert_eq!(drums.name, "drums");
        assert!(drums.children[0].is_folder()); // folders first
        assert_eq!(
            drums.samples(),
            vec![
                ("kick", "factory:drums/kit/kick"),
                ("snare", "factory:drums/snare")
            ]
        );
    }

    #[test]
    fn project_and_library_references() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("samples")).unwrap();
        std::fs::write(dir.path().join("samples/a.wav"), b"x").unwrap();
        let rel = scan(dir.path(), &RefStyle::RelativeTo(dir.path().into()), 4);
        assert_eq!(rel[0].children[0].samples()[0].1, "samples/a.wav");
        let abs = scan(dir.path(), &RefStyle::Absolute, 4);
        assert!(abs[0].children[0].samples()[0].1.ends_with("samples/a.wav"));
        assert!(!abs[0].children[0].samples()[0].1.contains('\\'));
    }

    #[test]
    fn presets_group_by_category() {
        let t = synth_presets(&["bass/sub", "lead/saw", "bass/acid"]);
        assert_eq!(
            t.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["bass", "lead"]
        );
        assert_eq!(t[0].children[0].name, "acid");
        assert_eq!(
            instrument_for(&t[0].children[0].kind),
            Some(InstrumentSpec::Synth {
                preset: Some("bass/acid".into()),
                params: Default::default()
            })
        );
    }

    #[test]
    fn names_are_tidied() {
        assert_eq!(channel_name("hat_closed"), "Hat closed");
        assert_eq!(channel_name("kick"), "Kick");
    }

    #[test]
    fn the_real_factory_folder_scans() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let tree = factory_samples(&assets);
        let all: Vec<_> = tree.iter().flat_map(|n| n.samples()).collect();
        assert!(all.iter().any(|(_, r)| *r == "factory:drums/haus808/kick"));
    }
}
