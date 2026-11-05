//! Opcode names: aliases (§14), key-valued opcodes and note names (§3.3), and the
//! classification of opcodes the mapper did not use (§15).

use super::lex::is_digits;

/// The canonical name for an alias (§14), or the name itself.
pub fn canonical(name: &str) -> String {
    let simple = match name {
        "loopmode" => Some("loop_mode"),
        "loopstart" => Some("loop_start"),
        "loopend" => Some("loop_end"),
        "filtype" => Some("fil_type"),
        "bendup" => Some("bend_up"),
        "benddown" => Some("bend_down"),
        "bendstep" => Some("bend_step"),
        "polyphony_group" => Some("group"),
        _ => None,
    };
    if let Some(s) = simple {
        return s.into();
    }
    // numbered aliases: (old prefix, canonical prefix)
    const NUMBERED: &[(&str, &str)] = &[
        ("gain_cc", "gain_oncc"),
        ("volume_oncc", "gain_oncc"),
        ("pitch_cc", "pitch_oncc"),
        ("tune_oncc", "pitch_oncc"),
        ("tune_cc", "pitch_oncc"),
        ("cutoff_cc", "cutoff_oncc"),
        ("offset_cc", "offset_oncc"),
        ("delay_cc", "delay_oncc"),
        ("amplfo_depthcc", "amplfo_depth_oncc"),
        ("fillfo_depthcc", "fillfo_depth_oncc"),
        ("pitchlfo_depthcc", "pitchlfo_depth_oncc"),
        // SPEC-GAP: §14 does not list the curve aliases; the curve follows its target's alias.
        ("volume_curvecc", "gain_curvecc"),
        ("tune_curvecc", "pitch_curvecc"),
    ];
    for (old, new) in NUMBERED {
        if let Some(n) = name.strip_prefix(old)
            && is_digits(n)
        {
            return format!("{new}{n}");
        }
    }
    for stage in ["delay", "start", "attack", "hold", "decay", "sustain", "release"] {
        let old = format!("ampeg_{stage}cc");
        if let Some(n) = name.strip_prefix(&old)
            && is_digits(n)
        {
            return format!("ampeg_{stage}_oncc{n}");
        }
    }
    name.into()
}

/// Opcodes whose value is a key (note names and the control offsets apply, §3.3).
pub fn is_key_opcode(name: &str) -> bool {
    matches!(
        name,
        "key"
            | "lokey"
            | "hikey"
            | "pitch_keycenter"
            | "sw_lokey"
            | "sw_hikey"
            | "sw_last"
            | "sw_lolast"
            | "sw_hilast"
            | "sw_down"
            | "sw_up"
            | "sw_previous"
            | "sw_default"
            | "xfin_lokey"
            | "xfin_hikey"
            | "xfout_lokey"
            | "xfout_hikey"
            | "amp_keycenter"
            | "fil_keycenter"
            | "fil2_keycenter"
            | "pan_keycenter"
    )
}

/// A key: a number, or a note name `letter [# | b] octave` with C4 = 60 (§3.3). Not clamped.
pub fn parse_key(v: &str) -> Option<i64> {
    let v = v.trim();
    if let Ok(x) = v.parse::<f64>() {
        return x.is_finite().then_some(x.trunc() as i64);
    }
    let mut chars = v.chars();
    let letter = chars.next()?.to_ascii_lowercase();
    let base = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (acc, oct) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') if rest[1..].parse::<i64>().is_ok() => (-1, &rest[1..]),
        _ => (0, rest),
    };
    let octave: i64 = oct.parse().ok()?;
    if !(-2..=10).contains(&octave) {
        return None;
    }
    Some(12 * (octave + 1) + base + acc)
}

/// What to say about an opcode the mapper did not use.
pub enum Leftover {
    /// In the spec's tables as "may skip" or "skip": reported as skipped.
    Skip(&'static str),
    /// Kept as data in the instrument (`extra`), reported as kept.
    Data,
    /// Not in the spec at all.
    Unknown,
}

fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(is_digits)
}

/// `lfoN_<rest>` with N digits: Some(rest).
pub fn lfo_rest(name: &str) -> Option<(u32, &str)> {
    let r = name.strip_prefix("lfo")?;
    let n = r.bytes().take_while(|b| b.is_ascii_digit()).count();
    let idx = r[..n].parse().ok()?;
    let rest = r[n..].strip_prefix('_')?;
    Some((idx, rest))
}

/// Classifies an opcode the mapper left unused.
pub fn leftover(name: &str) -> Leftover {
    use Leftover::*;
    const SKIP_NAMES: &[(&str, &str)] = &[
        ("lopolyaft", "poly aftertouch conditions"),
        ("hipolyaft", "poly aftertouch conditions"),
        ("loprog", "program-change conditions"),
        ("hiprog", "program-change conditions"),
        ("lobpm", "tempo conditions"),
        ("hibpm", "tempo conditions"),
        ("lotimer", "timer conditions"),
        ("hitimer", "timer conditions"),
        ("sw_vel", "previous-note velocity"),
        ("sample_fadeout", "sample-end fades"),
        ("sync_beats", "tempo-synced starts"),
        ("sync_offset", "tempo-synced starts"),
        ("delay_beats", "tempo-synced starts"),
        ("stop_beats", "tempo-synced starts"),
        ("waveguide", "waveguide playback"),
        ("load_mode", "loader hints"),
        ("load_start", "loader hints"),
        ("load_end", "loader hints"),
        ("sample_quality", "loader hints"),
        ("loop_count", "counted loops"),
        ("loop_tune", "loop tuning"),
        ("loop_end_offset", "loop end offsets"),
        ("pan_law", "pan laws"),
        ("phase", "polarity inversion"),
        ("bend_step", "pitch-bend steps"),
        ("bend_smooth", "pitch-bend smoothing"),
        ("ampeg_dynamic", "re-evaluated envelope controllers"),
        ("ampeg_attack_shape", "envelope curve shapes"),
        ("ampeg_decay_shape", "envelope curve shapes"),
        ("ampeg_release_shape", "envelope curve shapes"),
        ("cutoff_chanaft", "aftertouch filter modulation"),
        ("cutoff_polyaft", "aftertouch filter modulation"),
        ("fil_gain", "shelf and peak filter gain"),
        ("fil2_gain", "shelf and peak filter gain"),
        ("off_shape", "choke shapes"),
        ("off_curve", "choke shapes"),
        ("polyphony_stealing", "stealing hints"),
        ("note_selfmask", "note self-masking"),
        ("rt_dead", "accepted and ignored: a release region plays on its own"),
        ("decim", "region decimation"),
        ("bitred", "region bit reduction"),
        ("sostenuto_sw", "the sostenuto pedal"),
        ("effect1", "effect sends without a matching <effect>"),
        ("effect2", "effect sends without a matching <effect>"),
        ("effect3", "effect sends without a matching <effect>"),
        ("effect4", "effect sends without a matching <effect>"),
    ];
    if let Some((_, why)) = SKIP_NAMES.iter().find(|(n, _)| *n == name) {
        return Skip(why);
    }
    const SKIP_NUMBERED: &[(&str, &str)] = &[
        ("stop_locc", "CC-stopped regions"),
        ("stop_hicc", "CC-stopped regions"),
        ("delay_oncc", "controller-modulated delay"),
        ("loop_startcc", "controller-modulated loops"),
        ("loop_lengthcc", "controller-modulated loops"),
        ("width_oncc", "controller-modulated width"),
        ("position_oncc", "controller-modulated position"),
        ("amp_velcurve_", "custom velocity curves"),
    ];
    if let Some((_, why)) = SKIP_NUMBERED.iter().find(|(p, _)| numbered(name, p)) {
        return Skip(why);
    }
    if name.contains("_smoothcc") || name.contains("_stepcc") {
        return Skip("controller smoothing and steps");
    }
    if name.starts_with("ampeg_")
        && (name.contains("_oncc") || name.contains("cc") || name.ends_with("_shape"))
    {
        return Skip("controller-modulated envelope stages");
    }
    for fam in ["amplfo_", "fillfo_", "pitchlfo_"] {
        if let Some(r) = name.strip_prefix(fam)
            && (r.contains("chanaft")
                || r.contains("polyaft")
                || r.starts_with("freqcc")
                || r.starts_with("freq_oncc")
                || r.starts_with("depth_oncc"))
        {
            return Skip("controller- and aftertouch-modulated LFOs");
        }
    }
    if let Some((_, rest)) = lfo_rest(name) {
        let skip = [
            "count", "steps", "smooth", "offset", "ratio", "scale", "decim", "bitred", "drive",
        ];
        if skip.iter().any(|s| rest.starts_with(s))
            || rest.starts_with("step")
            || rest.contains("_lfo")
            || rest.starts_with("eq")
            || rest.starts_with("noise")
        {
            return Skip("LFO features the sampler does not have");
        }
    }
    if name.starts_with("hint_") {
        return Skip("player hints");
    }
    if name.starts_with("var") && name[3..].starts_with(|c: char| c.is_ascii_digit()) {
        return Skip("variable modulators");
    }
    if name.starts_with("oscillator") {
        return Skip("wavetable oscillators");
    }
    if name.starts_with("noise_") {
        return Skip("region noise");
    }
    if name.starts_with("eq") && name[2..].starts_with(|c: char| c.is_ascii_digit()) {
        return Skip("region EQ bands");
    }
    if numbered(name, "label_cc")
        || numbered(name, "label_key")
        || numbered(name, "label_output")
        || matches!(name, "md5" | "image" | "region_label")
    {
        return Data;
    }
    Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names() {
        for (s, k) in [
            ("c4", 60),
            ("C4", 60),
            ("c#4", 61),
            ("Eb-1", 3),
            ("G9", 127),
            ("c-1", 0),
            ("b4", 71),
            ("bb4", 70),
            ("60", 60),
            ("60.7", 60),
        ] {
            assert_eq!(parse_key(s), Some(k), "{s}");
        }
        for s in ["h4", "c", "", "c#", "x#4"] {
            assert_eq!(parse_key(s), None, "{s}");
        }
    }

    #[test]
    fn aliases() {
        for (a, b) in [
            ("loopmode", "loop_mode"),
            ("volume_oncc7", "gain_oncc7"),
            ("gain_cc1", "gain_oncc1"),
            ("tune_cc3", "pitch_oncc3"),
            ("ampeg_attackcc1", "ampeg_attack_oncc1"),
            ("amplfo_depthcc1", "amplfo_depth_oncc1"),
            ("polyphony_group", "group"),
            ("lokey", "lokey"),
        ] {
            assert_eq!(canonical(a), b);
        }
    }
}
