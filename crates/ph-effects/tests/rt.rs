//! Real-time contract tests for every built-in effect: no allocation on the audio thread,
//! determinism, block-size independence, and no denormal slowdown.

mod common;

use common::*;
use ph_node::{Processor, Quality};

#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn make(name: &'static str) -> impl Fn() -> Box<dyn Processor> {
    move || ph_effects::builtin(name).unwrap()
}

macro_rules! effect_tests {
    ($($m:ident => $name:literal),* $(,)?) => {$(
        mod $m {
            use super::*;

            #[test]
            fn no_alloc_under_stress() {
                stress_no_alloc(&make($name), true, 0);
            }

            #[test]
            fn deterministic() {
                assert_deterministic(&make($name), true, 0);
            }

            #[test]
            fn block_size_independent() {
                assert_block_independent(&make($name), true, 0, Quality::Live);
                assert_block_independent(&make($name), true, 0, Quality::High);
            }

            #[test]
            fn no_denormal_slowdown() {
                assert_no_denormal_slowdown(&make($name), false);
            }
        }
    )*};
}

effect_tests! {
    eq => "eq",
    compressor => "compressor",
    limiter => "limiter",
    reverb => "reverb",
    delay => "delay",
    filter => "filter",
    saturator => "saturator",
    multiband => "multiband",
    gate => "gate",
    convolution => "convolution",
    chorus => "chorus",
    flanger => "flanger",
    phaser => "phaser",
    bitcrush => "bitcrush",
    stereo => "stereo",
    analyzer => "analyzer",
}
