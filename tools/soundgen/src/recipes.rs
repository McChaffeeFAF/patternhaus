//! The factory recipes: every sound is a voice function plus an explicit parameter list.

use crate::voices::{self, P};

pub type Params = Vec<(&'static str, f64)>;

pub struct Recipe {
    /// Path relative to `assets/samples/`, without extension (`drums/haus808/kick`).
    pub path: String,
    /// Voice name recorded in provenance.
    pub name: &'static str,
    pub params: Params,
    pub func: fn(&P) -> Vec<f64>,
}

impl Recipe {
    /// Path of the WAV relative to `assets/`.
    pub fn asset_path(&self) -> String {
        format!("samples/{}.wav", self.path)
    }

    /// FNV-1a hash of the recipe name and parameters.
    pub fn params_hash(&self) -> String {
        let mut text = String::from(self.name);
        for (k, v) in &self.params {
            text.push_str(&format!(";{k}={v:?}"));
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in text.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    pub fn render(&self) -> Vec<f64> {
        (self.func)(&P(&self.params))
    }
}

/// Defaults overridden by `over`; an override naming an unknown key is a bug.
fn mk(defaults: &[(&'static str, f64)], over: &[(&'static str, f64)]) -> Params {
    for (k, _) in over {
        assert!(defaults.iter().any(|(d, _)| d == k), "unknown param {k}");
    }
    defaults
        .iter()
        .map(|(k, v)| {
            (
                *k,
                over.iter().find(|(o, _)| o == k).map_or(*v, |(_, o)| *o),
            )
        })
        .collect()
}

const KICK: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 0.8),
    ("f_start", 150.0),
    ("f_end", 50.0),
    ("sweep_tau", 0.02),
    ("amp_tau", 0.2),
    ("click", 0.1),
    ("click_tau", 0.003),
    ("click_lp", 3000.0),
    ("drive", 1.0),
    ("p2_level", 0.0),
    ("p2_ratio", 2.3),
    ("p2_tau", 0.05),
    ("release", 0.05),
];

const MEMBRANE: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 0.5),
    ("f0", 180.0),
    ("drop", 0.15),
    ("drop_tau", 0.02),
    ("modes", 2.0),
    ("rolloff", 0.5),
    ("tau", 0.09),
    ("damp", 1.0),
    ("noise", 0.0),
    ("noise_tau", 0.13),
    ("noise_hp", 1800.0),
    ("noise_lp", 9000.0),
    ("click", 0.2),
    ("drive", 1.0),
    ("release", 0.05),
];

const RIM: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 0.12),
    ("f1", 1750.0),
    ("f2", 480.0),
    ("tau", 0.01),
    ("noise", 0.5),
    ("noise_bp", 3500.0),
    ("noise_tau", 0.004),
    ("drive", 1.5),
    ("release", 0.02),
];

const CLAP: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 0.45),
    ("bursts", 4.0),
    ("spacing", 0.0105),
    ("burst_tau", 0.0025),
    ("tail_tau", 0.09),
    ("bp", 1050.0),
    ("q", 1.4),
    ("drive", 1.0),
    ("release", 0.06),
];

const METAL: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 0.25),
    ("pitch", 1.0),
    ("noise", 0.1),
    ("tau", 0.03),
    ("hp", 7000.0),
    ("bp", 9000.0),
    ("q", 0.9),
    ("bp_mix", 0.5),
    ("lp", 12000.0),
    ("thud", 0.0),
    ("release", 0.03),
];

const PLATE: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 0.3),
    ("modes", 60.0),
    ("f_lo", 4000.0),
    ("f_hi", 15000.0),
    ("tau", 0.04),
    ("noise", 0.2),
    ("noise_tau", 0.03),
    ("hp", 5000.0),
    ("release", 0.03),
];

const COWBELL: &[(&str, f64)] = &[
    ("dur", 0.6),
    ("f1", 540.0),
    ("f2", 800.0),
    ("bp", 2640.0),
    ("q", 1.2),
    ("mix_a", 0.6),
    ("tau_a", 0.006),
    ("tau_b", 0.11),
    ("drive", 1.5),
    ("release", 0.08),
];

const SHAKER: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 0.25),
    ("bp", 6000.0),
    ("q", 0.8),
    ("hp", 3000.0),
    ("rise", 0.012),
    ("tau", 0.045),
    ("release", 0.04),
];

const CLAVE: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 0.3),
    ("f1", 2500.0),
    ("ratio", 1.58),
    ("tau", 0.03),
    ("drive", 1.2),
    ("release", 0.05),
];

const SUB: &[(&str, f64)] = &[
    ("dur", 2.4),
    ("f", 65.406),
    ("settle", 1.5),
    ("settle_tau", 0.012),
    ("attack", 0.004),
    ("tau", 0.75),
    ("drive", 1.6),
    ("release", 0.25),
];

fn r(path: &str, name: &'static str, params: Params, func: fn(&P) -> Vec<f64>) -> Recipe {
    Recipe {
        path: path.to_string(),
        name,
        params,
        func,
    }
}

fn haus808() -> Vec<Recipe> {
    let k = "drums/haus808";
    let tom = |f0: f64, tau: f64, dur: f64, seed: f64| {
        mk(
            MEMBRANE,
            &[
                ("seed", seed),
                ("dur", dur),
                ("f0", f0),
                ("drop", 0.55),
                ("drop_tau", 0.03),
                ("modes", 1.0),
                ("tau", tau),
                ("click", 0.12),
                ("drive", 1.2),
            ],
        )
    };
    let mut v = vec![
        r(
            &format!("{k}/kick"),
            "kick",
            mk(
                KICK,
                &[
                    ("seed", 11.0),
                    ("dur", 1.0),
                    ("f_start", 130.0),
                    ("f_end", 47.0),
                    ("sweep_tau", 0.018),
                    ("amp_tau", 0.3),
                    ("click", 0.12),
                    ("click_lp", 2500.0),
                    ("drive", 1.4),
                    ("release", 0.1),
                ],
            ),
            voices::kick,
        ),
        r(
            &format!("{k}/snare"),
            "membrane",
            mk(
                MEMBRANE,
                &[
                    ("seed", 12.0),
                    ("f0", 185.0),
                    ("noise", 0.75),
                    ("click", 0.25),
                ],
            ),
            voices::membrane,
        ),
        r(
            &format!("{k}/rim"),
            "rim",
            mk(RIM, &[("seed", 13.0)]),
            voices::rim,
        ),
        r(
            &format!("{k}/clap"),
            "clap",
            mk(CLAP, &[("seed", 14.0)]),
            voices::clap,
        ),
        r(
            &format!("{k}/hat_closed"),
            "metal",
            mk(METAL, &[("seed", 15.0), ("tau", 0.028), ("noise", 0.08)]),
            voices::metal,
        ),
        r(
            &format!("{k}/hat_open"),
            "metal",
            mk(
                METAL,
                &[
                    ("seed", 16.0),
                    ("dur", 1.8),
                    ("tau", 0.28),
                    ("noise", 0.08),
                    ("release", 0.15),
                ],
            ),
            voices::metal,
        ),
        r(
            &format!("{k}/hat_pedal"),
            "metal",
            mk(
                METAL,
                &[
                    ("seed", 17.0),
                    ("dur", 0.35),
                    ("tau", 0.06),
                    ("hp", 5500.0),
                    ("bp", 7000.0),
                    ("thud", 0.25),
                    ("release", 0.05),
                ],
            ),
            voices::metal,
        ),
        r(
            &format!("{k}/tom_low"),
            "membrane",
            tom(90.0, 0.28, 1.0, 18.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/tom_mid"),
            "membrane",
            tom(130.0, 0.25, 0.8, 19.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/tom_high"),
            "membrane",
            tom(190.0, 0.2, 0.7, 20.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/crash"),
            "metal",
            mk(
                METAL,
                &[
                    ("seed", 21.0),
                    ("dur", 2.4),
                    ("pitch", 0.9),
                    ("noise", 0.25),
                    ("tau", 0.7),
                    ("hp", 3500.0),
                    ("bp", 6500.0),
                    ("q", 0.6),
                    ("release", 0.3),
                ],
            ),
            voices::metal,
        ),
        r(
            &format!("{k}/cowbell"),
            "cowbell",
            mk(COWBELL, &[]),
            voices::cowbell,
        ),
    ];
    // Long tuned subs, one per octave of C.
    for (name, hz) in [
        ("sub_c1", 32.703_195_662_574_83),
        ("sub_c2", 65.406_391_325_149_66),
        ("sub_c3", 130.812_782_650_299_3),
    ] {
        v.push(r(
            &format!("{k}/{name}"),
            "sub",
            mk(SUB, &[("f", hz)]),
            voices::sub,
        ));
    }
    v
}

fn haus909() -> Vec<Recipe> {
    let k = "drums/haus909";
    let tom = |f0: f64, tau: f64, dur: f64, seed: f64| {
        mk(
            MEMBRANE,
            &[
                ("seed", seed),
                ("dur", dur),
                ("f0", f0),
                ("drop", 0.6),
                ("drop_tau", 0.04),
                ("modes", 2.0),
                ("rolloff", 1.0),
                ("tau", tau),
                ("noise", 0.06),
                ("noise_tau", 0.08),
                ("click", 0.35),
                ("drive", 1.5),
            ],
        )
    };
    let hat = |seed: f64, tau: f64, dur: f64, release: f64| {
        mk(
            METAL,
            &[
                ("seed", seed),
                ("pitch", 1.12),
                ("noise", 0.5),
                ("tau", tau),
                ("dur", dur),
                ("hp", 6500.0),
                ("bp", 9500.0),
                ("release", release),
            ],
        )
    };
    vec![
        r(
            &format!("{k}/kick"),
            "kick",
            mk(
                KICK,
                &[
                    ("seed", 31.0),
                    ("dur", 0.7),
                    ("f_start", 260.0),
                    ("f_end", 54.0),
                    ("sweep_tau", 0.012),
                    ("amp_tau", 0.19),
                    ("click", 0.5),
                    ("click_tau", 0.0025),
                    ("click_lp", 4000.0),
                    ("drive", 3.5),
                    ("release", 0.08),
                ],
            ),
            voices::kick,
        ),
        r(
            &format!("{k}/snare"),
            "membrane",
            mk(
                MEMBRANE,
                &[
                    ("seed", 32.0),
                    ("dur", 0.5),
                    ("f0", 200.0),
                    ("drop", 0.2),
                    ("drop_tau", 0.015),
                    ("rolloff", 0.4),
                    ("tau", 0.06),
                    ("damp", 1.5),
                    ("noise", 1.2),
                    ("noise_tau", 0.17),
                    ("noise_hp", 1200.0),
                    ("noise_lp", 11000.0),
                    ("click", 0.5),
                    ("drive", 2.0),
                ],
            ),
            voices::membrane,
        ),
        r(
            &format!("{k}/rim"),
            "rim",
            mk(
                RIM,
                &[
                    ("seed", 33.0),
                    ("f1", 1900.0),
                    ("f2", 520.0),
                    ("tau", 0.008),
                    ("noise", 0.8),
                    ("noise_bp", 4200.0),
                ],
            ),
            voices::rim,
        ),
        r(
            &format!("{k}/clap"),
            "clap",
            mk(
                CLAP,
                &[
                    ("seed", 34.0),
                    ("dur", 0.55),
                    ("bursts", 5.0),
                    ("spacing", 0.0095),
                    ("burst_tau", 0.003),
                    ("tail_tau", 0.14),
                    ("bp", 1250.0),
                    ("q", 1.2),
                    ("drive", 1.5),
                ],
            ),
            voices::clap,
        ),
        r(
            &format!("{k}/hat_closed"),
            "metal",
            hat(35.0, 0.035, 0.25, 0.03),
            voices::metal,
        ),
        r(
            &format!("{k}/hat_open"),
            "metal",
            hat(36.0, 0.35, 1.8, 0.15),
            voices::metal,
        ),
        r(
            &format!("{k}/hat_pedal"),
            "metal",
            {
                let mut p = hat(37.0, 0.07, 0.35, 0.05);
                p.iter_mut().for_each(|(key, v)| {
                    if *key == "thud" {
                        *v = 0.2;
                    }
                });
                p
            },
            voices::metal,
        ),
        r(
            &format!("{k}/tom_low"),
            "membrane",
            tom(100.0, 0.3, 0.9, 38.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/tom_mid"),
            "membrane",
            tom(150.0, 0.26, 0.8, 39.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/tom_high"),
            "membrane",
            tom(220.0, 0.22, 0.7, 40.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/crash"),
            "metal",
            mk(
                METAL,
                &[
                    ("seed", 41.0),
                    ("dur", 2.6),
                    ("pitch", 1.05),
                    ("noise", 0.6),
                    ("tau", 0.9),
                    ("hp", 3000.0),
                    ("bp", 7000.0),
                    ("q", 0.6),
                    ("release", 0.3),
                ],
            ),
            voices::metal,
        ),
        r(
            &format!("{k}/shaker"),
            "shaker",
            mk(SHAKER, &[("seed", 42.0)]),
            voices::shaker,
        ),
    ]
}

fn acoustic() -> Vec<Recipe> {
    let k = "drums/acoustic-ish";
    let tom = |f0: f64, tau: f64, seed: f64| {
        mk(
            MEMBRANE,
            &[
                ("seed", seed),
                ("dur", 1.4),
                ("f0", f0),
                ("drop", 0.12),
                ("drop_tau", 0.05),
                ("modes", 6.0),
                ("rolloff", 1.2),
                ("tau", tau),
                ("damp", 0.6),
                ("click", 0.3),
                ("drive", 1.0),
                ("release", 0.15),
            ],
        )
    };
    let hat = |seed: f64, tau: f64, dur: f64, release: f64| {
        mk(
            PLATE,
            &[
                ("seed", seed),
                ("tau", tau),
                ("dur", dur),
                ("release", release),
            ],
        )
    };
    vec![
        r(
            &format!("{k}/kick"),
            "kick",
            mk(
                KICK,
                &[
                    ("seed", 51.0),
                    ("dur", 0.6),
                    ("f_start", 120.0),
                    ("f_end", 62.0),
                    ("sweep_tau", 0.012),
                    ("amp_tau", 0.13),
                    ("p2_level", 0.2),
                    ("p2_ratio", 2.3),
                    ("p2_tau", 0.05),
                    ("click", 0.25),
                    ("click_tau", 0.004),
                    ("click_lp", 1800.0),
                    ("drive", 1.1),
                    ("release", 0.08),
                ],
            ),
            voices::kick,
        ),
        r(
            &format!("{k}/snare"),
            "membrane",
            mk(
                MEMBRANE,
                &[
                    ("seed", 52.0),
                    ("dur", 0.6),
                    ("f0", 195.0),
                    ("drop", 0.06),
                    ("modes", 6.0),
                    ("rolloff", 1.0),
                    ("tau", 0.09),
                    ("damp", 0.8),
                    ("noise", 0.6),
                    ("noise_tau", 0.2),
                    ("noise_hp", 2500.0),
                    ("click", 0.3),
                    ("release", 0.1),
                ],
            ),
            voices::membrane,
        ),
        r(
            &format!("{k}/rim"),
            "rim",
            mk(
                RIM,
                &[
                    ("seed", 53.0),
                    ("dur", 0.18),
                    ("f1", 1200.0),
                    ("f2", 380.0),
                    ("tau", 0.02),
                    ("noise", 0.4),
                    ("noise_bp", 2500.0),
                    ("noise_tau", 0.02),
                    ("drive", 1.0),
                ],
            ),
            voices::rim,
        ),
        r(
            &format!("{k}/clap"),
            "clap",
            mk(
                CLAP,
                &[
                    ("seed", 54.0),
                    ("dur", 0.5),
                    ("bursts", 7.0),
                    ("spacing", 0.013),
                    ("burst_tau", 0.004),
                    ("tail_tau", 0.1),
                    ("bp", 900.0),
                    ("q", 0.9),
                ],
            ),
            voices::clap,
        ),
        r(
            &format!("{k}/hat_closed"),
            "plate",
            hat(55.0, 0.04, 0.3, 0.04),
            voices::plate,
        ),
        r(
            &format!("{k}/hat_open"),
            "plate",
            hat(56.0, 0.35, 1.6, 0.2),
            voices::plate,
        ),
        r(
            &format!("{k}/hat_pedal"),
            "plate",
            {
                let mut p = hat(57.0, 0.07, 0.35, 0.06);
                p.iter_mut().for_each(|(key, v)| {
                    if *key == "f_hi" {
                        *v = 9000.0;
                    }
                });
                p
            },
            voices::plate,
        ),
        r(
            &format!("{k}/tom_low"),
            "membrane",
            tom(85.0, 0.5, 58.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/tom_mid"),
            "membrane",
            tom(120.0, 0.45, 59.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/tom_high"),
            "membrane",
            tom(170.0, 0.4, 60.0),
            voices::membrane,
        ),
        r(
            &format!("{k}/crash"),
            "plate",
            mk(
                PLATE,
                &[
                    ("seed", 61.0),
                    ("dur", 3.0),
                    ("modes", 90.0),
                    ("f_lo", 1500.0),
                    ("f_hi", 15000.0),
                    ("tau", 1.4),
                    ("noise", 0.25),
                    ("noise_tau", 0.5),
                    ("hp", 2500.0),
                    ("release", 0.4),
                ],
            ),
            voices::plate,
        ),
        r(
            &format!("{k}/clave"),
            "clave",
            mk(CLAVE, &[("seed", 62.0)]),
            voices::clave,
        ),
    ]
}

/// Every factory sound, in a stable order.
pub fn all() -> Vec<Recipe> {
    let mut v = haus808();
    v.extend(haus909());
    v.extend(acoustic());
    v.extend(impulse_responses());
    v
}

const REVERB_IR: &[(&str, f64)] = &[
    ("seed", 1.0),
    ("dur", 1.0),
    ("rt60", 1.0),
    ("damp", 1.0),
    ("pre", 0.0),
    ("er", 0.0),
    ("er_win", 0.05),
    ("spring", 0.0),
    ("spring_period", 0.037),
    ("release", 0.1),
];

/// Reverb impulse responses for the convolution reverb (`factory:ir/<name>`).
fn impulse_responses() -> Vec<Recipe> {
    let ir = |name: &str, over: &[(&'static str, f64)]| {
        r(
            &format!("ir/{name}"),
            "reverb_ir",
            mk(REVERB_IR, over),
            voices::reverb_ir,
        )
    };
    vec![
        ir(
            "room",
            &[
                ("seed", 101.0),
                ("dur", 0.6),
                ("rt60", 0.45),
                ("damp", 5.0),
                ("pre", 0.002),
                ("er", 14.0),
                ("er_win", 0.025),
                ("release", 0.1),
            ],
        ),
        ir(
            "hall",
            &[
                ("seed", 102.0),
                ("dur", 3.2),
                ("rt60", 2.2),
                ("damp", 0.9),
                ("pre", 0.012),
                ("er", 24.0),
                ("er_win", 0.08),
                ("release", 0.4),
            ],
        ),
        ir(
            "plate",
            &[
                ("seed", 103.0),
                ("dur", 2.2),
                ("rt60", 1.5),
                ("damp", 0.35),
                ("release", 0.3),
            ],
        ),
        ir(
            "spring",
            &[
                ("seed", 104.0),
                ("dur", 2.4),
                ("rt60", 1.8),
                ("damp", 1.6),
                ("spring", 1.0),
                ("spring_period", 0.037),
                ("release", 0.3),
            ],
        ),
    ]
}
