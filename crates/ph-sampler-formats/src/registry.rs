//! The importer hook (M3/T6): a session's set of importers, built-ins plus any registered at
//! runtime by a separately built component (`docs/formats/importer-hook.md`).
//!
//! Design: the set is a plain value, not global state. A front end builds it once
//! ([`Importers::builtin`], then [`Importers::register`]) and passes it to the studio, whose
//! host hands it to commands (`ph_commands::Host::importers`) and to the UI. Two sessions in
//! one process can therefore have different formats, and tests register importers without
//! touching anyone else's.

use crate::{ImportError, ImportInput, Imported, Importer, importers};
use ph_core::{
    HausInstrument, SampleRef, validate_group, validate_instrument_header, validate_zone,
};
use std::path::Path;
use std::sync::Arc;

/// Why [`Importers::register`] refused an importer. Registration never replaces a format
/// another importer already reads: the first importer of an extension keeps it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    #[error("an importer needs a format id (lower-case letters, digits, `-` or `_`), not {0:?}")]
    BadFormat(String),
    #[error("the {format} importer declares no extensions")]
    NoExtensions { format: String },
    #[error("the {format} importer's extension {extension:?} must be lower case, without the dot")]
    BadExtension { format: String, extension: String },
    #[error("an importer with the format id {0} is already registered")]
    DuplicateFormat(String),
    #[error("the {format} importer claims .{extension}, which the {owner} importer already reads")]
    ExtensionTaken {
        format: String,
        extension: String,
        owner: String,
    },
}

/// The importers of one session: the built-ins, then registered ones, in that order.
#[derive(Clone)]
pub struct Importers {
    list: Vec<Arc<dyn Importer>>,
}

/// A `&'static` built-in importer, shared as an `Arc`.
struct Builtin(&'static dyn Importer);

impl Importer for Builtin {
    fn format(&self) -> &'static str {
        self.0.format()
    }
    fn extensions(&self) -> &'static [&'static str] {
        self.0.extensions()
    }
    fn version(&self) -> u32 {
        self.0.version()
    }
    fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError> {
        self.0.import(input)
    }
}

impl Default for Importers {
    /// The built-in importers.
    fn default() -> Self {
        Self::builtin()
    }
}

/// Two sets are equal when they read the same formats (format id and version), in the same
/// order. Format ids are unique within a set.
impl PartialEq for Importers {
    fn eq(&self, other: &Self) -> bool {
        self.list.len() == other.list.len()
            && self
                .list
                .iter()
                .zip(&other.list)
                .all(|(a, b)| a.format() == b.format() && a.version() == b.version())
    }
}

impl std::fmt::Debug for Importers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.list.iter().map(|i| i.format()))
            .finish()
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

impl Importers {
    /// The importers built into Patternhaus (SFZ, SF2/SF3, DecentSampler, EXS24).
    pub fn builtin() -> Self {
        Self {
            list: importers()
                .iter()
                .map(|i| Arc::new(Builtin(*i)) as Arc<dyn Importer>)
                .collect(),
        }
    }

    /// No importers at all (tests of the hook itself).
    pub fn empty() -> Self {
        Self { list: Vec::new() }
    }

    /// Adds an importer after the ones already in the set. Refused, leaving the set as it
    /// was, when its format id or extensions are malformed, its format id is taken, or it
    /// claims an extension another importer already reads.
    pub fn register(&mut self, importer: impl Importer + 'static) -> Result<(), RegisterError> {
        self.register_shared(Arc::new(importer))
    }

    /// [`Importers::register`] for an importer that is already shared.
    pub fn register_shared(&mut self, importer: Arc<dyn Importer>) -> Result<(), RegisterError> {
        let format = importer.format();
        if !valid_id(format) {
            return Err(RegisterError::BadFormat(format.to_string()));
        }
        if importer.extensions().is_empty() {
            return Err(RegisterError::NoExtensions {
                format: format.into(),
            });
        }
        if self.list.iter().any(|i| i.format() == format) {
            return Err(RegisterError::DuplicateFormat(format.into()));
        }
        for (n, ext) in importer.extensions().iter().enumerate() {
            if !valid_id(ext) {
                return Err(RegisterError::BadExtension {
                    format: format.into(),
                    extension: ext.to_string(),
                });
            }
            let owner = self
                .list
                .iter()
                .find(|i| i.extensions().contains(ext))
                .map(|i| i.format())
                .or_else(|| importer.extensions()[..n].contains(ext).then_some(format));
            if let Some(owner) = owner {
                return Err(RegisterError::ExtensionTaken {
                    format: format.into(),
                    extension: ext.to_string(),
                    owner: owner.into(),
                });
            }
        }
        self.list.push(importer);
        Ok(())
    }

    /// [`Importers::register`], chained.
    pub fn with(mut self, importer: impl Importer + 'static) -> Result<Self, RegisterError> {
        self.register(importer)?;
        Ok(self)
    }

    /// Every importer, built-ins first.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Importer> {
        self.list.iter().map(|i| i.as_ref())
    }

    /// The format ids, in order.
    pub fn formats(&self) -> Vec<&'static str> {
        self.iter().map(|i| i.format()).collect()
    }

    /// The importer for a file name's extension (case-insensitive).
    pub fn importer_for(&self, path: &Path) -> Option<&dyn Importer> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        self.iter().find(|i| i.extensions().contains(&ext.as_str()))
    }

    /// Every extension some importer reads (lower case, no dot): the browser's file filter
    /// and drop-to-create use this list.
    pub fn extensions(&self) -> Vec<&'static str> {
        self.iter()
            .flat_map(|i| i.extensions().iter().copied())
            .collect()
    }

    /// Imports a file by its extension, through [`import_with`].
    pub fn import_path(&self, path: &Path) -> Result<Imported, ImportError> {
        let importer = self
            .importer_for(path)
            .ok_or_else(|| ImportError::Unsupported {
                file: path.to_string_lossy().into_owned(),
                reason: format!(
                    "not an instrument format the sampler reads ({})",
                    self.extensions().join(", ")
                ),
            })?;
        import_with(importer, ImportInput::Path(path))
    }
}

/// Runs one importer the way the host does: a panic (a bug: importers must never panic) is
/// caught and becomes an error naming the format, and an accepted result must pass
/// [`check_imported`].
pub fn import_with(
    importer: &dyn Importer,
    input: ImportInput<'_>,
) -> Result<Imported, ImportError> {
    let file = input.name();
    let format = importer.format();
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| importer.import(input)))
        .map_err(|_| ImportError::Unsupported {
        file: file.clone(),
        reason: format!("the {format} importer failed unexpectedly (a bug in that importer)"),
    })??;
    check_imported(&out).map_err(|reason| ImportError::Unsupported {
        file,
        reason: format!("the {format} importer produced an invalid instrument: {reason}"),
    })?;
    Ok(out)
}

/// What every accepted import must be: a valid instrument (the model's checks) whose zones
/// sit in existing groups and play only samples the import lists. The fuzz targets assert
/// this for every importer; hook importers should too.
pub fn check_imported(out: &Imported) -> Result<(), String> {
    let i: &HausInstrument = &out.instrument;
    validate_instrument_header(i).map_err(|e| e.to_string())?;
    for g in i.groups.values() {
        validate_group(g).map_err(|e| e.to_string())?;
    }
    for z in i.zones.values() {
        validate_zone(z).map_err(|e| e.to_string())?;
        if !i.groups.contains_key(&z.group) {
            return Err(format!("zone {} is in a missing group", z.id));
        }
        if let SampleRef::File(name) = &z.sample
            && !out.samples.iter().any(|s| &s.name == name)
        {
            return Err(format!("zone {} plays the unlisted sample {name}", z.id));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImportReport;

    /// A configurable fake importer.
    struct Fake {
        format: &'static str,
        exts: &'static [&'static str],
        panics: bool,
    }

    impl Importer for Fake {
        fn format(&self) -> &'static str {
            self.format
        }
        fn extensions(&self) -> &'static [&'static str] {
            self.exts
        }
        fn version(&self) -> u32 {
            1
        }
        fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError> {
            if self.panics {
                panic!("fake importer bug");
            }
            Ok(Imported {
                instrument: HausInstrument::new(&input.name()),
                samples: Vec::new(),
                report: ImportReport {
                    format: self.format.into(),
                    file: input.name(),
                    ..Default::default()
                },
            })
        }
    }

    fn fake(format: &'static str, exts: &'static [&'static str]) -> Fake {
        Fake {
            format,
            exts,
            panics: false,
        }
    }

    #[test]
    fn registered_importers_join_lookup_and_extensions() {
        let mut set = Importers::builtin();
        set.register(fake("toy", &["toy", "toyz"])).unwrap();
        assert_eq!(set.formats(), ["sfz", "sf2", "dspreset", "exs", "toy"]);
        assert!(set.extensions().ends_with(&["exs", "toy", "toyz"]));
        assert_eq!(
            set.importer_for(Path::new("a/B.TOYZ")).unwrap().format(),
            "toy"
        );
        // the built-in set is untouched: no global state
        assert!(
            Importers::builtin()
                .importer_for(Path::new("a.toy"))
                .is_none()
        );
        assert_eq!(format!("{:?}", Importers::empty()), "[]");
    }

    #[test]
    fn registration_refuses_clashes_and_bad_ids() {
        let mut set = Importers::builtin();
        assert_eq!(
            set.register(fake("mine", &["sfz"])),
            Err(RegisterError::ExtensionTaken {
                format: "mine".into(),
                extension: "sfz".into(),
                owner: "sfz".into()
            })
        );
        assert_eq!(
            set.register(fake("sfz", &["sfz2"])),
            Err(RegisterError::DuplicateFormat("sfz".into()))
        );
        assert!(matches!(
            set.register(fake("", &["x"])),
            Err(RegisterError::BadFormat(_))
        ));
        assert!(matches!(
            set.register(fake("x", &[])),
            Err(RegisterError::NoExtensions { .. })
        ));
        assert!(matches!(
            set.register(fake("x", &[".x"])),
            Err(RegisterError::BadExtension { .. })
        ));
        assert!(matches!(
            set.register(fake("x", &["X"])),
            Err(RegisterError::BadExtension { .. })
        ));
        assert!(matches!(
            set.register(fake("x", &["a", "a"])),
            Err(RegisterError::ExtensionTaken { .. })
        ));
        assert_eq!(set.formats().len(), 4, "refusals leave the set unchanged");
    }

    #[test]
    fn a_panicking_importer_is_an_error_not_a_crash() {
        let f = Fake {
            format: "bad",
            exts: &["bad"],
            panics: true,
        };
        let e = import_with(
            &f,
            ImportInput::Bytes {
                name: "x.bad",
                bytes: b"",
            },
        )
        .unwrap_err();
        assert!(
            e.to_string().contains("bad importer failed unexpectedly"),
            "{e}"
        );
        let ok = import_with(
            &fake("toy", &["toy"]),
            ImportInput::Bytes {
                name: "x.toy",
                bytes: b"",
            },
        );
        assert!(ok.is_ok());
    }
}
