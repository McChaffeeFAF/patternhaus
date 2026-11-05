//! Fuzz targets, one per importer, in the style of the FLP fuzzers (`ph-formats`
//! `flp::tests`): bounded proptests over random bytes that run in CI with the normal test
//! suite. Every outcome must be an error or an instrument that passes the model's checks;
//! none may panic. While an importer is a stub (M3/T0) the targets only prove the wiring;
//! each track adds its generated fixtures as seeds (`fuzz_<format>_mutated`) when it lands.

use ph_core::{HausInstrument, validate_group, validate_instrument_header, validate_zone};
use ph_sampler_formats::*;
use proptest::prelude::*;

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

    #[test]
    fn fuzz_sf2(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        prop_assert!(run(&SoundFont, "fuzz.sf2", &body).is_ok());
        let riff = with_prefix(b"RIFF\x00\x10\x00\x00sfbkLIST", &body);
        prop_assert!(run(&SoundFont, "fuzz.sf2", &riff).is_ok());
    }

    #[test]
    fn fuzz_dspreset(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        prop_assert!(run(&DecentSampler, "fuzz.dspreset", &body).is_ok());
        let xml = with_prefix(b"<DecentSampler><groups><group><sample path=\"", &body);
        prop_assert!(run(&DecentSampler, "fuzz.dspreset", &xml).is_ok());
    }

    #[test]
    fn fuzz_exs(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        prop_assert!(run(&Exs, "fuzz.exs", &body).is_ok());
    }

    #[test]
    fn fuzz_kontakt(body in proptest::collection::vec(any::<u8>(), 0..800)) {
        for name in ["fuzz.nki", "fuzz.nkm", "fuzz.ncw"] {
            prop_assert!(run(&Kontakt, name, &body).is_ok());
        }
    }
}
