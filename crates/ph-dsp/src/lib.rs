//! Patternhaus DSP primitives.
//!
//! Small, allocation-free building blocks shared by the built-in instruments and effects.
//! Everything that runs per sample is real-time safe: constructors and `prepare`-style
//! methods may allocate, `next`/`process`/`tick` methods never do.
//!
//! Sources (public literature only):
//! - TPT/ZDF filters: V. Zavalishin, *The Art of VA Filter Design* (rev. 2.1.2).
//! - Biquads: R. Bristow-Johnson, *Cookbook formulae for audio EQ biquad filter coefficients*.
//! - PolyBLEP/PolyBLAMP: V. Välimäki et al., "Antialiasing oscillators in subtractive
//!   synthesis" (IEEE SPM 2007) and Esqueda, Välimäki, Bilbao, "Rounding corners with BLAMP"
//!   (DAFx-16).
//! - Halfband FIR and Kaiser window: Oppenheim & Schafer, *Discrete-Time Signal Processing*.
//! - Envelope curves: the one-pole "target overshoot" ADSR described in the public
//!   EarLevel Engineering articles.

#![allow(
    clippy::needless_range_loop,
    clippy::excessive_precision,
    clippy::should_implement_trait
)]

pub mod analysis;
pub mod delay;
pub mod dynamics;
pub mod env;
pub mod filter;
pub mod interp;
pub mod lfo;
pub mod math;
pub mod onset;
pub mod osc;
pub mod oversample;
pub mod smooth;
pub mod voice;

pub use math::*;
