//! One SF2 preset to the Haus Sampler model (`docs/formats/sf2.md` §5–§10).
//!
//! Every (preset zone × instrument zone) pair whose ranges meet becomes one native zone with
//! fully summed generators (§7); group-level settings (envelopes, filter, LFOs, choke,
//! sends, modulation routes) decide the Haus group (`crate::assemble`). Samples are cut out
//! of `smpl` (or decoded, SF3) and handed over as [`SampleSource::Audio`]: the caller
//! writes them into the instrument's `samples/` folder.

use super::gens::{self, *};
use super::riff::Sf2;
use crate::assemble::{Assembler, GroupDraft, clampf, safe_file, split_cents};
use crate::notes::Notes;
use crate::{ImportedSample, SampleSource};
use ph_core::*;
use ph_node::SampleData;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A sample cut out of the file, mono, with its loop relative to its first frame.
struct Extracted {
    data: Vec<f32>,
    rate: u32,
    loop_rel: (i64, i64),
    pitch: u8,
    correction: i8,
    /// Low four bits of the sample type: 1 mono, 2 right, 4 left, 8 linked.
    role: u16,
    link: u16,
    name: String,
}

fn timecents(v: i32) -> f64 {
    if v <= -32768 {
        0.0
    } else {
        2f64.powf(v as f64 / 1200.0)
    }
}

fn abs_cents_hz(c: f64) -> f64 {
    8.176 * 2f64.powf(c / 1200.0)
}

/// Cuts sample `idx` out of the file (§3, §6, §9).
fn extract(sf: &Sf2<'_>, idx: usize, notes: &mut Notes) -> Result<Extracted, String> {
    let h = sf.samples.get(idx).ok_or("no such sample")?;
    if h.kind & 0x8000 != 0 {
        return Err(if sf.info.rom.is_some() {
            format!("{} needs the sound ROM", h.name)
        } else {
            format!("{} needs a sound ROM the file does not name (irom)", h.name)
        });
    }
    let compressed = h.kind & 0x10 != 0;
    let mut role = h.kind & 0x0F;
    if !matches!(role, 1 | 2 | 4 | 8) || h.kind & !0x801F != 0 {
        notes.approx(
            "sample:type",
            format!("unknown sample type {:#06x} played as mono", h.kind),
        );
        role = 1;
    }
    let rate = if h.rate == 0 {
        notes.approx(
            "sample:rate",
            "a sample with rate 0 (illegal) is played at 44100 Hz",
        );
        44_100
    } else {
        h.rate
    };
    let (data, loop_rel) = if compressed {
        if !sf.is_sf3() {
            notes.approx(
                "sample:compressed",
                "a compressed sample in a file that is not SF3 was read as SF3",
            );
        }
        let (s, e) = (h.start as usize, h.end as usize);
        if e < s || e >= sf.smpl.len() {
            return Err(format!(
                "{}: unsupported SF3 variant (the offsets do not point at a stream)",
                h.name
            ));
        }
        let stream = &sf.smpl[s..=e];
        let head = &stream[..stream.len().min(80)];
        let has = |pat: &[u8]| head.windows(pat.len()).any(|w| w == pat);
        let ext = if stream.starts_with(b"OggS") && has(b"\x01vorbis") {
            "ogg"
        } else if stream.starts_with(b"OggS") && has(b"OpusHead") {
            return Err(format!(
                "{}: Opus-compressed samples are not supported",
                h.name
            ));
        } else if stream.starts_with(b"fLaC") {
            "flac"
        } else {
            return Err(format!(
                "{}: unsupported SF3 variant (no Ogg Vorbis or FLAC stream at its offset)",
                h.name
            ));
        };
        let d = ph_formats::decode::decode_bytes(stream, Some(ext))
            .map_err(|e| format!("{}: the compressed sample does not decode ({e})", h.name))?;
        if d.channels.len() != 1 {
            return Err(format!(
                "{}: a compressed sample must be mono, this one has {} channels",
                h.name,
                d.channels.len()
            ));
        }
        let data = d.channels.into_iter().next().unwrap_or_default();
        (data, (h.loop_start as i64, h.loop_end as i64))
    } else {
        let total = sf.smpl.len() / 2;
        let (s, e) = (h.start as usize, h.end as usize);
        if s >= e || e > total {
            return Err(format!(
                "{}: the sample lies outside the sample data",
                h.name
            ));
        }
        let data = (s..e)
            .map(|i| {
                let v = i16::from_le_bytes([sf.smpl[2 * i], sf.smpl[2 * i + 1]]) as i32;
                match sf.sm24 {
                    Some(lo) => {
                        ((v << 8) | lo.get(i).copied().unwrap_or(0) as i32) as f32 / 8_388_608.0
                    }
                    None => v as f32 / 32_768.0,
                }
            })
            .collect();
        (
            data,
            (
                h.loop_start as i64 - h.start as i64,
                h.loop_end as i64 - h.start as i64,
            ),
        )
    };
    if data.is_empty() {
        return Err(format!("{}: the sample is empty", h.name));
    }
    Ok(Extracted {
        data,
        rate,
        loop_rel,
        pitch: h.pitch,
        correction: h.correction,
        role,
        link: h.link,
        name: h.name.clone(),
    })
}

/// One voice: a (preset zone × instrument zone) pair.
struct Voice {
    pz: usize,
    inst: usize,
    sample: usize,
    keys: (u8, u8),
    vel: (u8, u8),
    vals: Vec<i32>,
    mods: Vec<Modulator>,
}

/// What modulators add: constant offsets on base values, and routes.
#[derive(Default)]
struct ModOut {
    pitch: f64,
    volume: f64,
    pan: f64,
    cutoff: f64,
    res: f64,
    routes: Vec<(ModSource, ModTarget, f64, ModCurve)>,
    /// Default #2 (velocity → cutoff) amount, applied only when the filter is on.
    vel_cutoff: f64,
    bend: Option<f64>,
}

fn dest_name(d: u16) -> &'static str {
    match d as usize {
        MOD_LFO_TO_PITCH => "modLfoToPitch",
        VIB_LFO_TO_PITCH => "vibLfoToPitch",
        MOD_ENV_TO_PITCH => "modEnvToPitch",
        MOD_LFO_TO_FC => "modLfoToFilterFc",
        MOD_ENV_TO_FC => "modEnvToFilterFc",
        MOD_LFO_TO_VOL => "modLfoToVolume",
        CHORUS => "chorusEffectsSend",
        REVERB => "reverbEffectsSend",
        _ => "this generator",
    }
}

/// Maps the voice's modulators (§8.4, §8.5). Unsupported ones are reported.
fn map_mods(mods: &[Modulator], notes: &mut Notes) -> ModOut {
    let mut o = ModOut::default();
    for m in mods {
        match m.default {
            Some(1) => {
                if m.amount != 0.0 {
                    notes.approx(
                        "modulator:velocity-to-volume",
                        "the velocity curve (default modulator 1) is played with the sampler's own velocity response",
                    );
                }
                continue;
            }
            Some(2) => {
                o.vel_cutoff = m.amount;
                continue;
            }
            Some(3) | Some(4) => {
                if m.amount != 0.0 {
                    notes.skip(
                        "modulator:vibrato-depth",
                        "mod wheel and channel pressure vibrato (default modulators 3, 4): the sampler cannot scale an LFO's depth by a controller yet",
                    );
                }
                continue;
            }
            Some(5) | Some(6) | Some(7) => {
                if m.amount != 0.0 {
                    notes.skip(
                        "modulator:channel-volume-pan",
                        "CC7 volume, CC10 pan and CC11 expression (default modulators 5–7) are left to the channel",
                    );
                }
                continue;
            }
            Some(8) | Some(9) => {
                if m.amount != 0.0 {
                    notes.skip(
                        "modulator:effects-sends",
                        "CC91 reverb and CC93 chorus send modulators (defaults 8, 9): sends are kept as data only",
                    );
                }
                continue;
            }
            Some(10) => {
                o.bend = Some(m.amount);
                continue;
            }
            _ => {}
        }
        if m.amount == 0.0 {
            continue;
        }
        let what = format!("modulator:{:#06x}->{}", m.src, m.dest);
        let (target, scale) = match m.dest as usize {
            ATTEN => (ModTarget::Volume, -0.1),
            PAN => (ModTarget::Pan, 1.0 / 500.0),
            FC => (ModTarget::Cutoff, 1.0),
            Q => (ModTarget::Resonance, 0.1),
            COARSE => (ModTarget::Pitch, 100.0),
            FINE => (ModTarget::Pitch, 1.0),
            _ if m.dest == PITCH_DEST => (ModTarget::Pitch, 1.0),
            _ => {
                notes.skip(
                    what,
                    format!(
                        "the sampler cannot modulate {} from a controller yet",
                        dest_name(m.dest)
                    ),
                );
                continue;
            }
        };
        if m.amt_src & 0x7F != 0 || m.amt_src & 0x80 != 0 {
            notes.skip(
                what,
                "amount sources (a second controller scaling a modulator) are not supported",
            );
            continue;
        }
        let idx = m.src & 0x7F;
        let cc = m.src & 0x80 != 0;
        let dir = m.src & 0x100 != 0;
        let bipolar = m.src & 0x200 != 0;
        let kind = m.src >> 10;
        // the model's source value c relates to the SF2 input x as x = p + q·c
        let (source, p, q) = if cc {
            (
                Some(ModSource::Cc {
                    controller: idx as u8,
                }),
                0.0,
                1.0,
            )
        } else {
            match idx {
                0 => (None, 1.0, 0.0),
                2 => (Some(ModSource::Velocity), 0.0, 1.0),
                3 => (Some(ModSource::Key), 0.468_75, 0.5),
                10 => (Some(ModSource::PolyPressure), 0.0, 1.0),
                13 => (Some(ModSource::Aftertouch), 0.0, 1.0),
                14 => (Some(ModSource::PitchBend), 0.5, 0.5),
                _ => {
                    notes.skip(what, "pitch-wheel sensitivity as a source is not supported");
                    continue;
                }
            }
        };
        let curve_fn = |x: f64| -> f64 {
            let concave = |x: f64| {
                if x >= 1.0 {
                    1.0
                } else {
                    (-(20.0 / 96.0) * ((1.0 - x) * (1.0 - x)).log10()).clamp(0.0, 1.0)
                }
            };
            match kind {
                1 => concave(x),
                2 => 1.0 - concave(1.0 - x),
                3 => {
                    if x >= 0.5 {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => x,
            }
        };
        let mut amount = m.amount;
        if m.transform == 2 {
            if bipolar {
                notes.skip(
                    what,
                    "the absolute-value transform on a bipolar source is not supported",
                );
                continue;
            }
            amount = amount.abs();
        }
        let Some(source) = source else {
            // no controller: a constant input of 1
            let u = if dir { 0.0 } else { 1.0 };
            let v = curve_fn(u);
            let out = if bipolar { 2.0 * v - 1.0 } else { v };
            add_offset(&mut o, target, amount * out * scale);
            continue;
        };
        let plain = p == 0.0 && q == 1.0;
        let (mut a, mut b, curve) = match (kind, plain) {
            (0, _) => (
                if dir { 1.0 - p } else { p },
                if dir { -q } else { q },
                ModCurve::Linear,
            ),
            (3, true) => (
                if dir { 1.0 } else { 0.0 },
                if dir { -1.0 } else { 1.0 },
                ModCurve::Switch,
            ),
            (1, true) | (2, true) => {
                notes.approx(
                    "modulator:curve",
                    "SF2 concave and convex curves are played with the sampler's √x and x² curves",
                );
                // concave(1 − x) = 1 − convex(x), and the mirror for convex
                let up = if kind == 1 {
                    ModCurve::Concave
                } else {
                    ModCurve::Convex
                };
                let down = if kind == 1 {
                    ModCurve::Convex
                } else {
                    ModCurve::Concave
                };
                if dir {
                    (1.0, -1.0, down)
                } else {
                    (0.0, 1.0, up)
                }
            }
            _ => {
                notes.approx(
                    "modulator:curve",
                    "a non-linear curve on a key or pitch-wheel source is played linearly",
                );
                (
                    if dir { 1.0 - p } else { p },
                    if dir { -q } else { q },
                    ModCurve::Linear,
                )
            }
        };
        if bipolar {
            a = 2.0 * a - 1.0;
            b *= 2.0;
        }
        add_offset(&mut o, target, amount * a * scale);
        o.routes.push((source, target, amount * b * scale, curve));
    }
    o
}

fn add_offset(o: &mut ModOut, t: ModTarget, v: f64) {
    match t {
        ModTarget::Pitch => o.pitch += v,
        ModTarget::Volume => o.volume += v,
        ModTarget::Pan => o.pan += v,
        ModTarget::Cutoff => o.cutoff += v,
        ModTarget::Resonance => o.res += v,
        ModTarget::SampleStart => {}
    }
}

fn env_time(v: i32, what: &str, notes: &mut Notes) -> f32 {
    let s = timecents(v);
    if s > 60.0 {
        notes.approx(
            format!("generator:{what}"),
            "envelope times longer than 60 s were cut to 60 s",
        );
    }
    s.min(60.0) as f32
}

/// The pair of zones that make one stereo sample, if `a` and `b` qualify (§6.2).
fn pairs(sf: &Sf2<'_>, a: &Extracted, ai: usize, b: &Extracted, bi: usize) -> bool {
    if !(a.role == 4 && b.role == 2) {
        return false;
    }
    let mutual = a.link as usize == bi && b.link as usize == ai;
    if !sf.is_sf3() {
        return mutual;
    }
    // SPEC-GAP: sf2.md §6.2: "do not join on links alone in SF3" read as: a mutual link or
    // matching L/R names, always with matching zones (checked by the caller).
    let stem = |n: &str, side: char| -> Option<String> {
        let t = n.trim_end();
        let lower = t.to_ascii_lowercase();
        let paren = format!("({side})");
        if lower.ends_with(&paren) {
            Some(t[..t.len() - 3].trim_end().to_string())
        } else if lower.ends_with(side) {
            Some(t[..t.len() - 1].trim_end().to_string())
        } else {
            None
        }
    };
    let names = matches!(
        (stem(&a.name, 'l'), stem(&b.name, 'r')),
        (Some(x), Some(y)) if x == y && !x.is_empty()
    );
    mutual || names
}

pub struct Mapped {
    pub instrument: HausInstrument,
    pub samples: Vec<ImportedSample>,
}

/// Maps preset `preset` of `sf`.
pub fn map(sf: &Sf2<'_>, preset: usize, notes: &mut Notes) -> Mapped {
    let p = &sf.presets[preset];
    let pz = gens::zones(
        &sf.pbag, p.bag, p.bag_end, &sf.pgen, &sf.pmod, true, &p.name, notes,
    );
    let mut inst_zones: BTreeMap<usize, gens::Zones> = BTreeMap::new();
    let mut voices: Vec<Voice> = Vec::new();
    let all = (0u8, 127u8);
    for (pzi, pl) in pz.local.iter().enumerate() {
        let Some(ii) = pl.target else { continue };
        let Some(ih) = sf.insts.get(ii) else { continue };
        let iz = inst_zones.entry(ii).or_insert_with(|| {
            gens::zones(
                &sf.ibag, ih.bag, ih.bag_end, &sf.igen, &sf.imod, false, &ih.name, notes,
            )
        });
        let pkey = pl
            .key
            .or(pz.global.as_ref().and_then(|g| g.key))
            .unwrap_or(all);
        let pvel = pl
            .vel
            .or(pz.global.as_ref().and_then(|g| g.vel))
            .unwrap_or(all);
        for il in &iz.local {
            let Some(si) = il.target else { continue };
            let ikey = il
                .key
                .or(iz.global.as_ref().and_then(|g| g.key))
                .unwrap_or(all);
            let ivel = il
                .vel
                .or(iz.global.as_ref().and_then(|g| g.vel))
                .unwrap_or(all);
            let (Some(keys), Some(vel)) = (intersect(pkey, ikey), intersect(pvel, ivel)) else {
                continue;
            };
            voices.push(Voice {
                pz: pzi,
                inst: ii,
                sample: si,
                keys,
                vel,
                vals: sum(pz.global.as_ref(), pl, iz.global.as_ref(), il),
                mods: voice_mods(pz.global.as_ref(), pl, iz.global.as_ref(), il, notes),
            });
        }
    }

    // samples, cut out once each
    let mut cache: BTreeMap<usize, Option<Extracted>> = BTreeMap::new();
    for v in &voices {
        if let std::collections::btree_map::Entry::Vacant(e) = cache.entry(v.sample) {
            match extract(sf, v.sample, notes) {
                Ok(x) => {
                    e.insert(Some(x));
                }
                Err(why) => {
                    notes.skip("sample", why);
                    e.insert(None);
                }
            }
        }
    }

    // stereo pairs (§6.2): left voice index -> right voice index
    let mut partner: BTreeMap<usize, usize> = BTreeMap::new();
    let mut taken = vec![false; voices.len()];
    for i in 0..voices.len() {
        for j in 0..voices.len() {
            if i == j || taken[i] || taken[j] {
                continue;
            }
            let (a, b) = (&voices[i], &voices[j]);
            if a.pz != b.pz || a.inst != b.inst || a.keys != b.keys || a.vel != b.vel {
                continue;
            }
            let (Some(Some(xa)), Some(Some(xb))) = (cache.get(&a.sample), cache.get(&b.sample))
            else {
                continue;
            };
            if pairs(sf, xa, a.sample, xb, b.sample) {
                partner.insert(i, j);
                taken[i] = true;
                taken[j] = true;
            }
        }
    }
    let joined_right: BTreeMap<usize, usize> = partner.iter().map(|(l, r)| (*r, *l)).collect();

    let mut inst = HausInstrument::new(if p.name.is_empty() {
        "SoundFont"
    } else {
        &p.name
    });
    for (k, v) in &sf.info.meta {
        if !v.is_empty() {
            inst.extra.insert(format!("sf2.info.{k}"), v.clone());
        }
    }
    let mut asm = Assembler::default();
    let mut samples: Vec<ImportedSample> = Vec::new();
    let mut sample_names: BTreeMap<(usize, Option<usize>), String> = BTreeMap::new();
    let mut used_names: BTreeMap<String, u32> = BTreeMap::new();
    let mut bend: Option<f64> = None;

    for (vi, v) in voices.iter().enumerate() {
        if partner.contains_key(&vi) {
            continue; // the left half: played through its right partner
        }
        let left = joined_right.get(&vi).map(|&l| &voices[l]);
        let Some(Some(x)) = cache.get(&v.sample) else {
            continue;
        };
        if left.is_none() && x.role != 1 {
            notes.approx(
                "sample:link",
                "a linked (stereo) sample without a matching partner zone plays as mono",
            );
        }
        let vals = &v.vals;
        let mo = map_mods(&v.mods, notes);
        if let Some(b) = mo.bend {
            match bend {
                None => bend = Some(b),
                Some(old) if old != b => notes.approx(
                    "modulator:pitch-wheel",
                    "zones with different pitch-bend ranges share the first one",
                ),
                _ => {}
            }
        }

        // the sample (joined with its left partner when it has one)
        let key = (v.sample, left.map(|l| l.sample));
        let name = match sample_names.get(&key) {
            Some(n) => n.clone(),
            None => {
                let (base, audio) = match left.and_then(|l| cache.get(&l.sample)) {
                    Some(Some(lx)) => {
                        let n = lx.data.len().max(x.data.len());
                        let mut l = lx.data.clone();
                        let mut r = x.data.clone();
                        l.resize(n, 0.0);
                        r.resize(n, 0.0);
                        notes.approx(
                            "stereo:join",
                            "a left/right sample pair was joined into one stereo zone (pitch, loop and level from the right sample)",
                        );
                        (format!("{} + {}", lx.name, x.name), vec![l, r])
                    }
                    _ => (x.name.clone(), vec![x.data.clone()]),
                };
                let base = safe_file(&base);
                let c = used_names.entry(base.to_lowercase()).or_insert(0);
                *c += 1;
                let n = if *c == 1 {
                    format!("samples/{base}.wav")
                } else {
                    format!("samples/{base} ({c}).wav")
                };
                samples.push(ImportedSample {
                    name: n.clone(),
                    source: SampleSource::Audio(Arc::new(SampleData {
                        sample_rate: x.rate as f64,
                        channels: audio,
                        loop_range: None,
                        root_key: None,
                    })),
                });
                sample_names.insert(key, n.clone());
                n
            }
        };

        // ---- group-level settings
        let ih = &sf.insts[v.inst];
        let gname = if ih.name.is_empty() {
            format!("Instrument {}", v.inst)
        } else {
            ih.name.clone()
        };
        let mut d = GroupDraft::new(&gname);
        let fixed_key = (vals[KEYNUM] >= 0).then_some(vals[KEYNUM]);
        let centre = fixed_key.unwrap_or((v.keys.0 as i32 + v.keys.1 as i32) / 2);
        if fixed_key.is_none()
            && v.keys.0 != v.keys.1
            && [
                KEY_TO_MOD_HOLD,
                KEY_TO_MOD_DECAY,
                KEY_TO_VOL_HOLD,
                KEY_TO_VOL_DECAY,
            ]
            .iter()
            .any(|&g| vals[g] != 0)
        {
            notes.approx(
                "generator:keynumToEnvelope",
                "key-dependent envelope times are taken at the middle key of each zone",
            );
        }
        let kt = |g: usize| vals[g] * (60 - centre);
        d.group.amp_env = Envelope {
            delay: env_time(vals[DELAY_VOL_ENV], "delayVolEnv", notes),
            attack: env_time(vals[34], "attackVolEnv", notes),
            hold: env_time(vals[35] + kt(KEY_TO_VOL_HOLD), "holdVolEnv", notes),
            decay: env_time(vals[36] + kt(KEY_TO_VOL_DECAY), "decayVolEnv", notes),
            sustain: 10f64
                .powf(-(vals[SUSTAIN_VOL] as f64) / 200.0)
                .clamp(0.0, 1.0) as f32,
            release: env_time(vals[38], "releaseVolEnv", notes).max(0.001),
        };
        let mod_env_used = vals[MOD_ENV_TO_PITCH] != 0 || vals[MOD_ENV_TO_FC] != 0;
        if mod_env_used {
            d.group.filter_env = Some(Envelope {
                delay: env_time(vals[DELAY_MOD_ENV], "delayModEnv", notes),
                attack: env_time(vals[26], "attackModEnv", notes),
                hold: env_time(vals[27] + kt(KEY_TO_MOD_HOLD), "holdModEnv", notes),
                decay: env_time(vals[28] + kt(KEY_TO_MOD_DECAY), "decayModEnv", notes),
                sustain: (1.0 - vals[29] as f64 / 1000.0).clamp(0.0, 1.0) as f32,
                release: env_time(vals[30], "releaseModEnv", notes).max(0.001),
            });
            d.route(
                ModSource::Envelope { index: 1 },
                ModTarget::Pitch,
                vals[MOD_ENV_TO_PITCH] as f64,
                ModCurve::Linear,
            );
        }
        // filter (§10): off when fully open, unresonant and unmodulated
        let fc = vals[FC] as f64 + mo.cutoff;
        let q = vals[Q] as f64 + mo.res * 10.0;
        let routes_filter = mo
            .routes
            .iter()
            .any(|r| matches!(r.1, ModTarget::Cutoff | ModTarget::Resonance));
        let filter_on = fc < 13500.0
            || q > 0.0
            || vals[MOD_ENV_TO_FC] != 0
            || vals[MOD_LFO_TO_FC] != 0
            || routes_filter;
        if filter_on {
            // default #2: amount × (1 − velocity) = amount − amount × velocity
            let base = fc + mo.vel_cutoff;
            d.route(
                ModSource::Velocity,
                ModTarget::Cutoff,
                -mo.vel_cutoff,
                ModCurve::Linear,
            );
            d.group.filter = Some(Filter {
                kind: FilterKind::LowPass12,
                cutoff: clampf(abs_cents_hz(base), 10.0, 22_000.0) as f32,
                resonance: clampf(q / 10.0, 0.0, 40.0) as f32,
                key_track: 0.0,
                env_depth: clampf(vals[MOD_ENV_TO_FC] as f64, -9600.0, 9600.0) as f32,
            });
            if q / 10.0 > 40.0 {
                notes.approx(
                    "generator:initialFilterQ",
                    "resonance above 40 dB was cut to 40 dB",
                );
            }
        }
        // LFOs (§10): mod LFO, vibrato LFO; triangle
        let lfo = |rate: usize, delay: usize, notes: &mut Notes| Lfo {
            shape: LfoShape::Triangle,
            rate: clampf(abs_cents_hz(vals[rate] as f64), 0.01, 100.0) as f32,
            delay: env_time(vals[delay], "lfoDelay", notes),
            fade: 0.0,
        };
        if vals[MOD_LFO_TO_PITCH] != 0 || vals[MOD_LFO_TO_FC] != 0 || vals[MOD_LFO_TO_VOL] != 0 {
            let index = d.group.lfos.len() as u8;
            d.group.lfos.push(lfo(FREQ_MOD_LFO, DELAY_MOD_LFO, notes));
            let s = ModSource::Lfo { index };
            d.route(
                s.clone(),
                ModTarget::Pitch,
                vals[MOD_LFO_TO_PITCH] as f64,
                ModCurve::Linear,
            );
            d.route(
                s.clone(),
                ModTarget::Cutoff,
                vals[MOD_LFO_TO_FC] as f64,
                ModCurve::Linear,
            );
            d.route(
                s,
                ModTarget::Volume,
                -(vals[MOD_LFO_TO_VOL] as f64) / 10.0,
                ModCurve::Linear,
            );
        }
        if vals[VIB_LFO_TO_PITCH] != 0 {
            let index = d.group.lfos.len() as u8;
            d.group.lfos.push(lfo(FREQ_VIB_LFO, DELAY_VIB_LFO, notes));
            d.route(
                ModSource::Lfo { index },
                ModTarget::Pitch,
                vals[VIB_LFO_TO_PITCH] as f64,
                ModCurve::Linear,
            );
        }
        if vals[EXCLUSIVE] != 0 {
            d.group.voice_limit = Some(1);
            d.group
                .extra
                .insert("sf2.exclusive_class".into(), vals[EXCLUSIVE].to_string());
        }
        for (g, key) in [(REVERB, "sf2.reverb_send"), (CHORUS, "sf2.chorus_send")] {
            if vals[g] != 0 {
                d.group.extra.insert(key.into(), vals[g].to_string());
                notes.kept(
                    "generator:effects-sends",
                    "reverb and chorus send levels are kept as data; the sampler has no sends yet",
                );
            }
        }
        for (s, t, depth, c) in &mo.routes {
            d.route(s.clone(), *t, *depth, *c);
        }

        // ---- pitch (§5.2)
        let root = if vals[ROOT_KEY] >= 0 {
            vals[ROOT_KEY]
        } else if x.pitch <= 127 {
            x.pitch as i32
        } else {
            60
        };
        let s = vals[SCALE] as f64;
        let c0 = vals[COARSE] as f64 * 100.0 + vals[FINE] as f64 + x.correction as f64 + mo.pitch;
        let (root2, cents) = if let Some(k) = fixed_key {
            d.route(ModSource::Key, ModTarget::Pitch, -6400.0, ModCurve::Linear);
            (60, (k - root) as f64 * s + c0)
        } else if s == 100.0 {
            (root, c0)
        } else {
            d.route(
                ModSource::Key,
                ModTarget::Pitch,
                (s - 100.0) * 64.0,
                ModCurve::Linear,
            );
            let r = (((root as f64 * s - (s - 100.0) * 60.0) / 100.0).round() as i32).clamp(0, 127);
            (
                r,
                r as f64 * 100.0 + (s - 100.0) * 60.0 - root as f64 * s + c0,
            )
        };
        let (tune, fine, clamped) = split_cents(cents);
        if clamped {
            notes.approx(
                "generator:coarseTune",
                "a pitch offset beyond ±48 semitones was cut",
            );
        }
        if vals[VELOCITY] >= 0 {
            notes.approx(
                "generator:velocity",
                "a fixed velocity (velocity generator): the played velocity still sets the level",
            );
        }

        // ---- addresses and loop (§5.1)
        let len = x.data.len() as i64;
        let c32 = 32768i64;
        let mut start = vals[START] as i64 + c32 * vals[START_COARSE] as i64;
        let mut end = len + vals[END] as i64 + c32 * vals[END_COARSE] as i64;
        start = start.clamp(0, len - 1);
        end = end.clamp(start + 1, len);
        let ls = x.loop_rel.0 + vals[LOOP_START] as i64 + c32 * vals[LOOP_START_COARSE] as i64;
        let le = x.loop_rel.1 + vals[LOOP_END] as i64 + c32 * vals[LOOP_END_COARSE] as i64;
        let looping = match vals[SAMPLE_MODES] & 3 {
            m @ (1 | 3) => {
                let (ls, le) = (ls.clamp(start, end), le.clamp(start, end));
                if le > ls {
                    Some(ZoneLoop {
                        mode: LoopMode::Forward,
                        start: ls as u64,
                        end: le as u64,
                        crossfade: 0,
                        on_release: if m == 3 {
                            LoopRelease::Exit
                        } else {
                            LoopRelease::Continue
                        },
                    })
                } else {
                    notes.approx("sample:loop", "an empty or reversed loop plays unlooped");
                    None
                }
            }
            _ => None,
        };

        let mut z = Zone::new(
            GroupId(0),
            SampleRef::File(name),
            KeyRange {
                lo: v.keys.0,
                hi: v.keys.1,
            },
            root2.clamp(0, 127) as u8,
        );
        z.velocity = VelRange {
            lo: v.vel.0,
            hi: v.vel.1,
        };
        z.tune = tune;
        z.fine = fine;
        // §8.4 [PH]: generator attenuation at 0.4 ×, modulators at full value
        z.volume_db = clampf(-0.04 * vals[ATTEN] as f64 + mo.volume, -144.0, 24.0) as f32;
        let pan = match left {
            Some(l) => (vals[PAN] as f64 + l.vals[PAN] as f64) / 2.0,
            None => vals[PAN] as f64,
        };
        z.pan = clampf(pan / 500.0 + mo.pan, -1.0, 1.0) as f32;
        z.start = start as u64;
        z.end = (end < len).then_some(end as u64);
        z.looping = looping;
        let gid = asm.group(&d);
        asm.zone(z, gid);
    }

    if let Some(b) = bend {
        let semis = clampf(b / 6350.0, 0.0, 48.0) as f32;
        inst.settings.bend_up = semis;
        inst.settings.bend_down = semis;
    }
    asm.finish(&mut inst);
    Mapped {
        instrument: inst,
        samples,
    }
}
