//! Real-time contract tests for the built-in instruments: no allocation on the audio
//! thread, determinism, block-size independence, no denormal slowdown.

#[path = "../../ph-effects/tests/common/mod.rs"]
mod common;
mod util;

use common::*;
use ph_node::{Processor, Quality};

#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn synth() -> Box<dyn Processor> {
    Box::new(ph_instruments::synth::Synth::new())
}

/// A synth patch that exercises everything: 3 oscillators, unison, both filters, sub,
/// noise, LFOs and the mod matrix.
fn busy_synth() -> Box<dyn Processor> {
    util::synth_with(&[
        ("osc2_level", 0.7),
        ("osc2_wave", 1.0),
        ("osc3_level", 0.5),
        ("osc3_wave", 2.0),
        ("sub_level", 0.4),
        ("noise_level", 0.1),
        ("f2_on", 1.0),
        ("f1_res", 0.8),
        ("f1_drive", 12.0),
        ("f1_env", 3.0),
        ("unison_voices", 4.0),
        ("mod1_src", 1.0),
        ("mod1_dst", 5.0),
        ("mod1_amt", 0.3),
        ("mod2_src", 2.0),
        ("mod2_dst", 9.0),
        ("mod2_amt", 0.5),
        ("lfo2_sync", 1.0),
        ("lfo2_retrig", 0.0),
        ("glide_mode", 2.0),
    ])
}

fn sampler() -> Box<dyn Processor> {
    util::sampler_with(util::test_sample(), &[("loop_mode", 1.0)])
}

fn sampler_plain() -> Box<dyn Processor> {
    util::sampler_with(util::test_sample(), &[])
}

#[test]
fn synth_no_alloc_under_stress() {
    stress_no_alloc(&synth, false, 400);
    stress_no_alloc(&busy_synth, false, 400);
}

#[test]
fn sampler_no_alloc_under_stress() {
    stress_no_alloc(&sampler, false, 600);
}

#[test]
fn synth_deterministic() {
    assert_deterministic(&busy_synth, false, 40);
}

#[test]
fn sampler_deterministic() {
    assert_deterministic(&sampler, false, 40);
}

#[test]
fn synth_block_size_independent() {
    assert_block_independent(&synth, false, 40, Quality::Live);
    assert_block_independent(&busy_synth, false, 40, Quality::Live);
    assert_block_independent(&busy_synth, false, 40, Quality::High);
}

#[test]
fn sampler_block_size_independent() {
    assert_block_independent(&sampler, false, 40, Quality::Live);
    assert_block_independent(&sampler_plain, false, 40, Quality::High);
}

#[test]
fn synth_no_denormal_slowdown() {
    assert_no_denormal_slowdown(&busy_synth, true);
}

#[test]
fn sampler_no_denormal_slowdown() {
    assert_no_denormal_slowdown(&sampler, true);
}
