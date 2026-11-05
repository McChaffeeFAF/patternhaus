//! Fuzz targets, one per importer, in the style of the FLP fuzzers (`ph-formats`
//! `flp::tests`): bounded proptests over random bytes that run in CI with the normal test
//! suite. Every outcome must be an error or an instrument that passes the model's checks;
//! none may panic. While an importer is a stub the targets only prove the wiring; each track
//! adds its generated fixtures as seeds (`fuzz_<format>_mutated`) when it lands. SFZ (M3/T3)
//! seeds `fuzz_sfz_mutated` with `sfz_seeds`; SF2/SF3 (M3/T4) seeds `fuzz_sf2_mutated` with
//! `sf2_seeds` (F-SF2-12), EXS (M3/T4) `fuzz_exs_mutated` with `exs_seeds` (F-EXS-10), and
//! DecentSampler (M3/T4) `fuzz_dspreset_mutated` with `ds_seeds`.

use ph_core::{HausInstrument, validate_group, validate_instrument_header, validate_zone};
use ph_sampler_formats::*;
use proptest::prelude::*;

mod ds_seeds;
mod exs_seeds;
mod sf2_seeds;
mod sfz_seeds;

/// An accepted import must be a valid instrument whose zones play only listed samples.
fn check(out: &Imported) -> Result<(), String> {
    let i: &HausInstrument = &out.instrument;
    validate_instrument_header(i).map_err(|e| e.to_string())?;
    for g in i.groups.values() {
        validate_group(g).map_err(|e| e.to_string())?;
    }
    for z in i.zones.values() {
        validate_zone(z).map_err(|e| e.to_string())?;
        if !i.groups.contains_key(&z.group) {
            return Err(format!("zone {} in missing group", z.id));
        }
        if let ph_core::SampleRef::File(name) = &z.sample
            && !out.samples.iter().any(|s| &s.name == name)
        {
            return Err(format!("zone {} plays unlisted sample {name}", z.id));
        }
    }
    Ok(())
}

fn run(importer: &dyn Importer, name: &str, bytes: &[u8]) -> Result<(), String> {
    match importer.import(ImportInput::Bytes { name, bytes }) {
        Ok(out) => check(&out),
        Err(_) => Ok(()),
    }
}

/// A folder with the samples the SFZ seeds play (`a.wav`, `s/a.wav`), made once.
fn sample_dir() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let d = tempfile::tempdir().unwrap();
        let tone: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.03).sin() * 0.5).collect();
        for p in ["a.wav", "s/a.wav"] {
            let path = d.path().join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            ph_formats::encode::write_wav(
                &path,
                &[&tone],
                48_000,
                ph_formats::encode::WavFormat::Pcm16,
            )
            .unwrap();
        }
        d
    })
    .path()
}

/// Applies byte edits: replace, insert, cut.
fn mutate(bytes: &mut Vec<u8>, edits: &[(u32, u8, u8)]) {
    for &(at, b, op) in edits {
        let at = at as usize % (bytes.len() + 1);
        match op {
            0 if at < bytes.len() => bytes[at] = b,
            1 => bytes.insert(at, b),
            _ => bytes.truncate(at.max(1)),
        }
    }
}

fn exs_seeds() -> &'static [Vec<u8>] {
    static S: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();
    S.get_or_init(exs_seeds::all)
}

fn sf2_seeds() -> &'static [Vec<u8>] {
    static S: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();
    S.get_or_init(sf2_seeds::all)
}

/// Bytes that start with something format-shaped, so the parser gets past its first check.
fn with_prefix(prefix: &[u8], body: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    v.extend_from_slice(body);
    v
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn fuzz_sfz(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        prop_assert!(run(&Sfz, "fuzz.sfz", &body).is_ok());
        let text = with_prefix(b"<region> sample=a.wav lokey=", &body);
        prop_assert!(run(&Sfz, "fuzz.sfz", &text).is_ok());
    }

    /// The SFZ seeds (malformed fixtures and well-formed shapes), mutated: bytes replaced,
    /// inserted and cut.
    #[test]
    fn fuzz_sfz_mutated(
        seed in 0usize..(sfz_seeds::MALFORMED.len() + sfz_seeds::VALID.len()),
        edits in proptest::collection::vec((any::<u16>(), any::<u8>(), 0u8..3), 0..12),
    ) {
        let text = sfz_seeds::MALFORMED
            .iter()
            .chain(sfz_seeds::VALID)
            .nth(seed)
            .unwrap();
        let mut bytes = text.as_bytes().to_vec();
        for (at, b, op) in edits {
            let at = at as usize % (bytes.len() + 1);
            match op {
                0 if at < bytes.len() => bytes[at] = b,
                1 => bytes.insert(at, b),
                _ => bytes.truncate(at.max(1)),
            }
        }
        prop_assert!(run(&Sfz, "fuzz.sfz", &bytes).is_ok());
        // with a folder holding the samples, so mapping runs on every mutation
        let dir = sample_dir();
        let path = dir.join("fuzz.sfz");
        std::fs::write(&path, &bytes).unwrap();
        if let Ok(out) = Sfz.import(ImportInput::Path(&path)) {
            prop_assert!(check(&out).is_ok(), "{:?}", check(&out));
        }
    }

    #[test]
    fn fuzz_sf2(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        prop_assert!(run(&SoundFont, "fuzz.sf2", &body).is_ok());
        let riff = with_prefix(b"RIFF\x00\x10\x00\x00sfbkLIST", &body);
        prop_assert!(run(&SoundFont, "fuzz.sf2", &riff).is_ok());
    }

    /// The SF2 seeds (F-SF2-12 and well-formed files), mutated: bytes replaced, inserted
    /// and cut.
    #[test]
    fn fuzz_sf2_mutated(
        seed in 0usize..14,
        edits in proptest::collection::vec((any::<u32>(), any::<u8>(), 0u8..3), 0..16),
    ) {
        let seeds = sf2_seeds();
        let mut bytes = seeds[seed % seeds.len()].clone();
        mutate(&mut bytes, &edits);
        prop_assert!(run(&SoundFont, "fuzz.sf2", &bytes).is_ok());
    }

    #[test]
    fn fuzz_dspreset(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        prop_assert!(run(&DecentSampler, "fuzz.dspreset", &body).is_ok());
        let xml = with_prefix(b"<DecentSampler><groups><group><sample path=\"", &body);
        prop_assert!(run(&DecentSampler, "fuzz.dspreset", &xml).is_ok());
    }

    /// The DecentSampler seeds (malformed presets and F-DS shapes), mutated, read with a
    /// folder holding their sample so mapping runs.
    #[test]
    fn fuzz_dspreset_mutated(
        seed in 0usize..(ds_seeds::MALFORMED.len() + ds_seeds::VALID.len()),
        edits in proptest::collection::vec((any::<u32>(), any::<u8>(), 0u8..3), 0..12),
    ) {
        let text = ds_seeds::MALFORMED.iter().chain(ds_seeds::VALID).nth(seed).unwrap();
        let mut bytes = text.as_bytes().to_vec();
        mutate(&mut bytes, &edits);
        prop_assert!(run(&DecentSampler, "fuzz.dspreset", &bytes).is_ok());
        let dir = sample_dir();
        let path = dir.join(format!("fuzz{seed}.dspreset"));
        std::fs::write(&path, &bytes).unwrap();
        if let Ok(out) = DecentSampler.import(ImportInput::Path(&path)) {
            prop_assert!(check(&out).is_ok(), "{:?}", check(&out));
        }
    }

    #[test]
    fn fuzz_exs(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        prop_assert!(run(&Exs, "fuzz.exs", &body).is_ok());
        let exs = with_prefix(
            b"\x01\x01\x00\x00\x28\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00TBOS", &body);
        prop_assert!(run(&Exs, "fuzz.exs", &exs).is_ok());
    }

    /// The EXS seeds (F-EXS-10 and well-formed files in both byte orders), mutated, read
    /// with a folder holding their sample so mapping runs.
    #[test]
    fn fuzz_exs_mutated(
        seed in 0usize..16,
        edits in proptest::collection::vec((any::<u32>(), any::<u8>(), 0u8..3), 0..16),
    ) {
        let seeds = exs_seeds();
        let mut bytes = seeds[seed % seeds.len()].clone();
        mutate(&mut bytes, &edits);
        prop_assert!(run(&Exs, "fuzz.exs", &bytes).is_ok());
        let dir = sample_dir();
        let path = dir.join(format!("fuzz{seed}.exs"));
        std::fs::write(&path, &bytes).unwrap();
        if let Ok(out) = Exs.import(ImportInput::Path(&path)) {
            prop_assert!(check(&out).is_ok(), "{:?}", check(&out));
        }
    }

    #[test]
    fn fuzz_kontakt(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        for name in ["fuzz.nki", "fuzz.nkm", "fuzz.ncw"] {
            prop_assert!(run(&Kontakt, name, &body).is_ok());
        }
    }
}
