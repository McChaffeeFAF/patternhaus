//! Time-stretch and pitch-shift for Patternhaus: a thin binding of
//! [Signalsmith Stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch) (MIT).
//!
//! # How it is built
//!
//! The C++ library is header-only. `vendor/` holds the two upstream headers it needs,
//! unmodified except for one marked line, with their licence texts:
//! - `vendor/signalsmith-stretch/signalsmith-stretch.h`: Signalsmith Stretch 1.3.1
//!   (MIT, © 2022 Geraint Luff / Signalsmith Audio Ltd), as shipped in the
//!   `signalsmith-stretch` 0.1.3 crate (upstream commit pinned by that crate);
//! - `vendor/signalsmith-linear/{fft,stft}.h`: Signalsmith Linear (MIT, © 2025 Signalsmith
//!   Audio), its FFT/STFT helper.
//!
//! **Patch:** `configure` reserves `bands/2 + 1` spectral peaks instead of `bands/2`, because
//! the peak finder can return `ceil(bands/2)` and must never reallocate inside `process`.
//!
//! `cpp/ph_stretch.cpp` is our own `extern "C"` wrapper, compiled with the `cc` crate as
//! C++14. The Rust declarations below are hand-written, so building needs a C++ compiler but
//! **not libclang** (no bindgen). See `docs/formats/m2-dependencies.md`.
//!
//! # Real-time use
//!
//! [`Stretch::new`] allocates (off the audio thread). [`Stretch::process`],
//! [`Stretch::seek`], [`Stretch::reset`] and [`Stretch::set_transpose`] do not allocate;
//! the wrapper counts C++ allocations per thread ([`cpp_allocations`]) so tests can prove it.
//! Output is deterministic: the random engine (used only for stretches slower than 0.5×) is
//! `std::minstd_rand` seeded by the caller, and the C++ is built without fast-math.

use std::ffi::c_void;
use std::ptr::NonNull;

unsafe extern "C" {
    fn ph_stretch_new(channels: i32, sample_rate: f32, preset: i32, seed: u32) -> *mut c_void;
    fn ph_stretch_free(h: *mut c_void);
    fn ph_stretch_reset(h: *mut c_void);
    fn ph_stretch_input_latency(h: *const c_void) -> i32;
    fn ph_stretch_output_latency(h: *const c_void) -> i32;
    fn ph_stretch_block_samples(h: *const c_void) -> i32;
    fn ph_stretch_interval_samples(h: *const c_void) -> i32;
    fn ph_stretch_set_transpose(h: *mut c_void, semitones: f32);
    fn ph_stretch_seek(h: *mut c_void, input: *const *const f32, frames: i32, rate: f64);
    fn ph_stretch_process(
        h: *mut c_void,
        input: *const *const f32,
        in_frames: i32,
        output: *const *mut f32,
        out_frames: i32,
    );
    fn ph_stretch_alloc_count() -> usize;
}

/// Most channels one instance handles (pointer arrays live on the stack).
pub const MAX_CHANNELS: usize = 8;

/// Signalsmith's two presets.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Preset {
    /// 120 ms blocks, 30 ms interval: the quality setting.
    Default,
    /// 100 ms blocks, 40 ms interval: about half the CPU.
    Cheaper,
}

/// One stretcher instance (one voice). `Send`, not `Sync`.
pub struct Stretch {
    h: NonNull<c_void>,
    channels: usize,
}

// SAFETY: the C++ object is only touched through `&mut self` (or `&self` for the const
// latency getters); it holds no thread-affine state.
unsafe impl Send for Stretch {}

impl Stretch {
    /// Creates and configures an instance. Allocates: never on the audio thread. `None` when
    /// the arguments are out of range or the C++ side fails.
    pub fn new(channels: usize, sample_rate: f64, preset: Preset, seed: u32) -> Option<Self> {
        if channels == 0 || channels > MAX_CHANNELS || sample_rate.is_nan() || sample_rate <= 1000.0
        {
            return None;
        }
        let p = match preset {
            Preset::Default => 0,
            Preset::Cheaper => 1,
        };
        // SAFETY: plain values in, an owned pointer (or null) out.
        let h = unsafe { ph_stretch_new(channels as i32, sample_rate as f32, p, seed) };
        NonNull::new(h).map(|h| Self { h, channels })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Input frames the analysis looks ahead of the output position.
    pub fn input_latency(&self) -> usize {
        // SAFETY: valid handle.
        unsafe { ph_stretch_input_latency(self.h.as_ptr()) }.max(0) as usize
    }

    /// Output frames before the first input frame's sound appears.
    pub fn output_latency(&self) -> usize {
        // SAFETY: valid handle.
        unsafe { ph_stretch_output_latency(self.h.as_ptr()) }.max(0) as usize
    }

    /// STFT block length in frames.
    pub fn block_samples(&self) -> usize {
        // SAFETY: valid handle.
        unsafe { ph_stretch_block_samples(self.h.as_ptr()) }.max(0) as usize
    }

    /// STFT hop in frames.
    pub fn interval_samples(&self) -> usize {
        // SAFETY: valid handle.
        unsafe { ph_stretch_interval_samples(self.h.as_ptr()) }.max(0) as usize
    }

    /// Real-time. Forgets all buffered audio.
    pub fn reset(&mut self) {
        // SAFETY: valid handle, exclusive access.
        unsafe { ph_stretch_reset(self.h.as_ptr()) }
    }

    /// Real-time. Pitch shift from now on, in semitones.
    pub fn set_transpose(&mut self, semitones: f64) {
        let s = if semitones.is_finite() {
            semitones.clamp(-48.0, 48.0)
        } else {
            0.0
        };
        // SAFETY: valid handle, exclusive access.
        unsafe { ph_stretch_set_transpose(self.h.as_ptr(), s as f32) }
    }

    /// Real-time. Pre-roll: feeds `input` (`[channel][frame]`, the frames just before the
    /// next input) without producing output, for playback at `rate` (input ÷ output frames).
    /// Ideally one block plus one interval long.
    pub fn seek(&mut self, input: &[&[f32]], rate: f64) {
        let Some(frames) = self.frames_of(input) else {
            return;
        };
        let ptrs = self.in_ptrs(input);
        // SAFETY: `ptrs[..channels]` point at slices of at least `frames` floats.
        unsafe { ph_stretch_seek(self.h.as_ptr(), ptrs.as_ptr(), frames as i32, rate) }
    }

    /// Real-time. Consumes all of `input` and produces all of `output` (both
    /// `[channel][frame]`, one slice per channel); the time ratio of the call is
    /// `input frames / output frames`. Mismatched channel counts are ignored (output left
    /// as is).
    pub fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let Some(in_frames) = self.frames_of(input) else {
            return;
        };
        if output.len() < self.channels {
            return;
        }
        let out_frames = output[..self.channels]
            .iter()
            .map(|c| c.len())
            .min()
            .unwrap_or(0);
        let ip = self.in_ptrs(input);
        let mut op = [std::ptr::null_mut::<f32>(); MAX_CHANNELS];
        for (p, c) in op.iter_mut().zip(output.iter_mut()) {
            *p = c.as_mut_ptr();
        }
        // SAFETY: every pointer covers at least the frame counts passed; output slices are
        // distinct `&mut` borrows.
        unsafe {
            ph_stretch_process(
                self.h.as_ptr(),
                ip.as_ptr(),
                in_frames as i32,
                op.as_ptr(),
                out_frames as i32,
            )
        }
    }

    fn frames_of(&self, input: &[&[f32]]) -> Option<usize> {
        if input.len() < self.channels {
            return None;
        }
        Some(
            input[..self.channels]
                .iter()
                .map(|c| c.len())
                .min()
                .unwrap_or(0),
        )
    }

    fn in_ptrs(&self, input: &[&[f32]]) -> [*const f32; MAX_CHANNELS] {
        let mut p = [std::ptr::null::<f32>(); MAX_CHANNELS];
        for (d, c) in p.iter_mut().zip(input.iter()) {
            *d = c.as_ptr();
        }
        p
    }
}

impl Drop for Stretch {
    fn drop(&mut self) {
        // SAFETY: owned handle, freed once.
        unsafe { ph_stretch_free(self.h.as_ptr()) }
    }
}

/// C++ heap allocations made on the calling thread so far (by this library's code).
pub fn cpp_allocations() -> usize {
    // SAFETY: reads a thread-local counter.
    unsafe { ph_stretch_alloc_count() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn sine(freq: f64, sr: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (0.5 * (TAU * freq * i as f64 / sr).sin()) as f32)
            .collect()
    }

    /// Zero-crossing frequency estimate over the steady middle of `x`.
    fn freq_of(x: &[f32], sr: f64) -> f64 {
        let (a, b) = (x.len() / 4, 3 * x.len() / 4);
        let mut first = None;
        let mut last = 0usize;
        let mut count = 0usize;
        for i in a + 1..b {
            if x[i - 1] < 0.0 && x[i] >= 0.0 {
                first.get_or_insert(i);
                last = i;
                count += 1;
            }
        }
        (count - 1) as f64 * sr / (last - first.unwrap()) as f64
    }

    #[test]
    fn configures_and_reports_latency() {
        let s = Stretch::new(2, 48_000.0, Preset::Default, 1).unwrap();
        assert_eq!(s.channels(), 2);
        assert_eq!(s.block_samples(), 5760);
        assert_eq!(s.interval_samples(), 1440);
        assert!(s.input_latency() > 0 && s.output_latency() > 0);
        assert!(Stretch::new(0, 48_000.0, Preset::Default, 1).is_none());
        assert!(Stretch::new(9, 48_000.0, Preset::Default, 1).is_none());
        assert!(Stretch::new(1, f64::NAN, Preset::Cheaper, 1).is_none());
    }

    #[test]
    fn stretching_keeps_pitch_and_transposing_moves_it() {
        let sr = 48_000.0;
        let x = sine(440.0, sr, 96_000);
        for (ratio, semis, want) in [(1.25, 0.0, 440.0), (0.8, 0.0, 440.0), (1.0, 12.0, 880.0)] {
            let mut s = Stretch::new(1, sr, Preset::Default, 7).unwrap();
            s.set_transpose(semis);
            let out_len = (x.len() as f64 / ratio) as usize;
            let mut y = vec![0.0f32; out_len];
            let block = 512usize;
            let mut o = 0;
            while o < out_len {
                let n = block.min(out_len - o);
                let i0 = (o as f64 * ratio) as usize;
                let i1 = (((o + n) as f64 * ratio) as usize).min(x.len());
                s.process(&[&x[i0..i1]], &mut [&mut y[o..o + n]]);
                o += n;
            }
            let f = freq_of(&y, sr);
            let cents = 1200.0 * (f / want).log2();
            assert!(cents.abs() < 5.0, "ratio {ratio} semis {semis}: {f} Hz");
        }
    }

    #[test]
    fn process_seek_and_reset_never_allocate() {
        let sr = 48_000.0;
        let x = sine(220.0, sr, 48_000);
        let at_start = cpp_allocations();
        let mut s = Stretch::new(2, sr, Preset::Default, 3).unwrap();
        assert!(
            cpp_allocations() > at_start,
            "the counter sees configure's allocations"
        );
        let mut l = vec![0.0f32; 300];
        let mut r = vec![0.0f32; 300];
        let before = cpp_allocations();
        s.seek(&[&x[..7200], &x[..7200]], 1.1);
        let mut pos = 7200;
        for k in 0..100 {
            let n_in = 300 + (k % 7) * 20;
            s.set_transpose((k % 5) as f64 - 2.0);
            s.process(
                &[&x[pos..pos + n_in], &x[pos..pos + n_in]],
                &mut [&mut l, &mut r],
            );
            pos = (pos + n_in) % 30_000;
            if k == 50 {
                s.reset();
            }
        }
        // silence takes the stretcher's silent path too
        let z = vec![0.0f32; 20_000];
        for _ in 0..10 {
            s.process(&[&z[..400], &z[..400]], &mut [&mut l, &mut r]);
        }
        assert_eq!(cpp_allocations(), before, "the C++ side allocated");
    }

    #[test]
    fn deterministic_for_a_seed() {
        let sr = 44_100.0;
        let x = sine(330.0, sr, 44_100);
        let run = || {
            // 0.4x: slower than 0.5x, so phase randomisation is in play
            let mut s = Stretch::new(1, sr, Preset::Cheaper, 42).unwrap();
            let mut y = vec![0.0f32; 4000];
            s.process(&[&x[..1600]], &mut [&mut y]);
            y
        };
        assert_eq!(run(), run());
    }
}
