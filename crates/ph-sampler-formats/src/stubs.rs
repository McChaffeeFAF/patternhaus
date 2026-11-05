//! The importers, stubbed until their track lands. Each keeps its final type name, format id,
//! extensions and version, so the registry, `sampler.import` and the fuzz targets are wired
//! now; the track replaces the body of `import` (and moves the type to its own module).

use crate::{ImportError, ImportInput, Imported, Importer};

macro_rules! stub {
    ($(#[$m:meta])* $name:ident, $format:literal, [$($ext:literal),+], $label:literal, $track:literal) => {
        $(#[$m])*
        #[derive(Copy, Clone, Debug, Default)]
        pub struct $name;

        impl Importer for $name {
            fn format(&self) -> &'static str {
                $format
            }
            fn extensions(&self) -> &'static [&'static str] {
                &[$($ext),+]
            }
            fn version(&self) -> u32 {
                0
            }
            fn import(&self, _input: ImportInput<'_>) -> Result<Imported, ImportError> {
                Err(ImportError::Unavailable {
                    format: $label.into(),
                    track: $track.into(),
                })
            }
        }
    };
}

stub!(
    /// DecentSampler `.dspreset` (XML): groups, samples, effects; UI controls kept as data.
    DecentSampler, "dspreset", ["dspreset"], "DecentSampler", "M3/T4"
);
stub!(
    /// Logic EXS24 `.exs`.
    Exs, "exs", ["exs"], "EXS24", "M3/T4"
);
stub!(
    /// Kontakt `.nki`/`.nkm`/`.nkx`/`.nkc`/`.ncw`, unencrypted only: zones, loops, NCW,
    /// monoliths first. Encrypted content is detected and refused, never decrypted.
    Kontakt, "nki", ["nki", "nkm", "nkx", "nkc", "ncw"], "Kontakt", "M3/T6"
);
