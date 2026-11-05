//! SFZ fuzz seeds shared by `sfz_fixtures.rs` and `fuzz.rs`: the malformed fixtures, plus
//! well-formed files that reach every part of the importer.

#![allow(dead_code)]

/// Malformed fixtures, also the fuzz corpus seeds (`tests/fuzz.rs`).
pub const MALFORMED: &[&str] = &[
    "<region",
    "<region> sample=",
    "<region> sample=a.wav lokey=c#99 hikey=zz pitch_keycenter=-500",
    "#define\n#define $\n#include\n#include \"unterminated\n<region> sample=$UNDEF",
    "<curve> curve_index=7 v999=1 v000=nan\n<group> gain_oncc1=1e400 gain_curvecc1=300",
    "/* never closed <region> sample=a.wav",
    "<region> sample=a.wav lovel=100 hivel=10 seq_length=0 seq_position=999 end=-1",
    "<region> sample=a.wav loop_start=900 loop_end=10 loop_mode=loop_continuous offset=99999999999",
    "<global><master><group><region><control><effect><midi><sample><unknown>",
    "sample=a.wav\0\u{fffd}<region>=\t=\n<<>>",
];

/// Well-formed seeds (shapes of the F-SFZ fixtures) for mutation.
pub const VALID: &[&str] = &[
    "<control> default_path=s/ octave_offset=1
<global> volume=-6
<master> transpose=2
<group> lokey=c3 hikey=b4 pitch_keycenter=60 seq_length=2
<region> sample=a.wav seq_position=1
<region> sample=a.wav seq_position=2 lorand=0 hirand=0.5",
    "#define $K 60
<group> key=$K sw_last=24 sw_lolast=24 sw_hilast=26 sw_default=24 sw_label=Long trigger=release rt_decay=6
<region> sample=a.wav loop_mode=loop_sustain loop_start=10 loop_end=99 loop_type=alternate loop_crossfade=0.1",
    "<curve> curve_index=9 v000=0 v064=0.9
<effect> type=reverb bus=fx2
<group> gain_oncc1=-6 gain_curvecc1=9 effect2=50 lfo1_freq=3 lfo1_wave=12 lfo1_pitch=20 amplfo_freq=4 amplfo_depth=2 cutoff=300 fil_type=lpf_6p fileg_depth=2400 fileg_attack=1
<region> sample=a.wav locc1=10 hicc1=90 xfin_locc1=0 xfin_hicc1=64 xfin_lovel=10 xfin_hivel=60 group=1 off_by=1",
];
