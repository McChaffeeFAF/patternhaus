//! Quality bar for the factory kits, verified by measurement (the generator cannot listen).

use soundgen::measure::*;
use soundgen::{recipes, render_i24, wav_bytes};
use std::sync::OnceLock;

struct Sound {
    path: String,
    i24: Vec<i32>,
    x: Vec<f64>,
}

fn sounds() -> &'static Vec<Sound> {
    static S: OnceLock<Vec<Sound>> = OnceLock::new();
    S.get_or_init(|| {
        recipes::all()
            .into_iter()
            .map(|r| {
                let i24 = render_i24(&r);
                let x = i24.iter().map(|s| *s as f64 / 8_388_607.0).collect();
                Sound {
                    path: r.path.clone(),
                    i24,
                    x,
                }
            })
            .collect()
    })
}

fn get(path: &str) -> &'static Sound {
    sounds()
        .iter()
        .find(|s| s.path == path)
        .unwrap_or_else(|| panic!("no sound {path}"))
}

const KITS: [&str; 3] = ["haus808", "haus909", "acoustic-ish"];

#[test]
fn every_kit_has_the_required_sounds() {
    for kit in KITS {
        let mut required = vec![
            "kick",
            "snare",
            "rim",
            "clap",
            "hat_closed",
            "hat_open",
            "hat_pedal",
            "tom_low",
            "tom_mid",
            "tom_high",
            "crash",
        ];
        required.push(match kit {
            "haus808" => "cowbell",
            "haus909" => "shaker",
            _ => "clave",
        });
        for name in required {
            get(&format!("drums/{kit}/{name}"));
        }
    }
    assert!(sounds().len() >= 36);
}

#[test]
fn peaks_are_minus_one_dbfs() {
    for s in sounds() {
        let p = db(peak(&s.x));
        assert!((p + 1.0).abs() < 0.01, "{}: peak {p:.3} dBFS", s.path);
    }
}

#[test]
fn no_dc_offset() {
    for s in sounds() {
        let d = dc_db(&s.x);
        assert!(d < -60.0, "{}: DC {d:.1} dBFS", s.path);
    }
}

#[test]
fn starts_and_ends_without_clicks() {
    for s in sounds() {
        assert_eq!(s.i24[0], 0, "{} must start at zero", s.path);
        assert_eq!(*s.i24.last().unwrap(), 0, "{} must end at zero", s.path);
        // 0.5 ms fade-in: the first 2 samples stay small.
        assert!(
            s.x[..2].iter().all(|v| v.abs() < 0.05),
            "{} fade-in",
            s.path
        );
        // The final millisecond is well below the peak (smooth fade to silence).
        let tail = peak(&s.x[s.x.len() - 48..]);
        assert!(db(tail) < -40.0, "{}: tail {:.1} dBFS", s.path, db(tail));
        // No sample-to-sample jump anywhere near the end.
        let step = s.x[s.x.len() - 480..]
            .windows(2)
            .fold(0.0f64, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(step < 0.06, "{}: end step {step}", s.path);
    }
}

#[test]
fn wav_is_48k_24bit_mono() {
    let s = get("drums/haus808/kick");
    let bytes = wav_bytes(&s.i24);
    let r = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
    let spec = r.spec();
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (1, 48_000, 24)
    );
    assert_eq!(r.len() as usize, s.i24.len());
}

#[test]
fn kick_fundamentals_are_40_to_70_hz() {
    for kit in KITS {
        let k = get(&format!("drums/{kit}/kick"));
        let f = dominant_hz(&k.x, 0.09, 0.40);
        println!("{kit} kick fundamental {f:.1} Hz");
        assert!((40.0..=70.0).contains(&f), "{kit}: {f:.1} Hz");
    }
}

#[test]
fn closed_hats_are_shorter_than_open_hats() {
    for kit in KITS {
        let c = active_len(&get(&format!("drums/{kit}/hat_closed")).x, -50.0);
        let o = active_len(&get(&format!("drums/{kit}/hat_open")).x, -50.0);
        let p = active_len(&get(&format!("drums/{kit}/hat_pedal")).x, -50.0);
        println!("{kit} hats: closed {c:.3} s, pedal {p:.3} s, open {o:.3} s");
        assert!(c < o * 0.5, "{kit}: closed {c} vs open {o}");
        assert!(c < 0.3, "{kit}: closed hat too long ({c} s)");
        assert!(p < o, "{kit}: pedal {p} vs open {o}");
    }
}

#[test]
fn tuned_808_subs_are_within_five_cents_of_c() {
    for (name, c_hz) in [
        ("sub_c1", 32.7032),
        ("sub_c2", 65.4064),
        ("sub_c3", 130.8128),
    ] {
        let s = get(&format!("drums/haus808/{name}"));
        let f = zero_cross_hz(&s.x, 0.25, 1.25);
        let cents = cents_from_c(f);
        println!("{name}: {f:.3} Hz, {cents:+.2} cents from C");
        assert!(cents.abs() < 5.0, "{name}: {cents:+.2} cents");
        assert!((f / c_hz - 1.0).abs() < 0.003, "{name} is the wrong C: {f}");
    }
}

#[test]
fn spectral_shape_is_plausible() {
    for kit in KITS {
        let kick = centroid_hz(&get(&format!("drums/{kit}/kick")).x);
        assert!(kick < 400.0, "{kit} kick centroid {kick:.0} Hz");
        for h in ["hat_closed", "hat_open", "hat_pedal"] {
            let c = centroid_hz(&get(&format!("drums/{kit}/{h}")).x);
            assert!(c > 3500.0, "{kit} {h} centroid {c:.0} Hz");
        }
        let snare = centroid_hz(&get(&format!("drums/{kit}/snare")).x);
        let kick_c = centroid_hz(&get(&format!("drums/{kit}/kick")).x);
        assert!(snare > 4.0 * kick_c, "{kit} snare not brighter than kick");
    }
}

#[test]
fn toms_ascend_in_pitch() {
    for kit in ["haus808", "haus909"] {
        let f = |n: &str| dominant_hz(&get(&format!("drums/{kit}/{n}")).x, 0.12, 0.35);
        let (lo, mid, hi) = (f("tom_low"), f("tom_mid"), f("tom_high"));
        println!("{kit} toms: {lo:.0} / {mid:.0} / {hi:.0} Hz");
        assert!(lo < mid && mid < hi, "{kit}: {lo} {mid} {hi}");
    }
}

#[test]
fn rendering_is_deterministic() {
    for r in recipes::all().iter().step_by(5) {
        assert_eq!(render_i24(r), get_i24(&r.path), "{}", r.path);
    }
}

fn get_i24(path: &str) -> Vec<i32> {
    get(path).i24.clone()
}

#[test]
fn files_are_small_and_finite() {
    let bytes: usize = sounds().iter().map(|s| s.i24.len() * 3).sum();
    assert!(bytes < 12_000_000, "kits total {bytes} bytes");
    for s in sounds() {
        assert!(s.x.iter().all(|v| v.is_finite()), "{}", s.path);
        assert!(s.x.len() as f64 / 48_000.0 <= 3.5, "{} too long", s.path);
    }
}
