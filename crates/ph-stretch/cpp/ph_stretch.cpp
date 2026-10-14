// Patternhaus: a minimal C ABI over Signalsmith Stretch (MIT, vendored in ../vendor).
//
// Written for Patternhaus (MIT OR Apache-2.0); the Rust side declares these functions by
// hand (crates/ph-stretch/src/lib.rs). Channel buffers are passed as arrays of pointers
// (non-interleaved), so no copy or allocation happens per call.
//
// No C++ exception may cross the ABI: every entry point catches.

#include <cstddef>
#include <cstdlib>
#include <new>
#include <random>

#include "signalsmith-stretch.h"

namespace {

using Stretch = signalsmith::stretch::SignalsmithStretch<float, std::minstd_rand>;

// Per thread, so tests running in parallel do not see each other's allocations.
thread_local size_t g_allocs = 0;

} // namespace

// Counting global allocation functions. They behave exactly like the defaults (malloc /
// free) and let tests prove that `process`, `seek` and `reset` never allocate
// (`ph_stretch_alloc_count`). Only this library's C++ code is linked into the binary.
void *operator new(std::size_t n) {
    ++g_allocs;
    if (void *p = std::malloc(n ? n : 1)) return p;
    throw std::bad_alloc();
}
void *operator new[](std::size_t n) {
    ++g_allocs;
    if (void *p = std::malloc(n ? n : 1)) return p;
    throw std::bad_alloc();
}
void *operator new(std::size_t n, const std::nothrow_t &) noexcept {
    ++g_allocs;
    return std::malloc(n ? n : 1);
}
void *operator new[](std::size_t n, const std::nothrow_t &) noexcept {
    ++g_allocs;
    return std::malloc(n ? n : 1);
}
void operator delete(void *p) noexcept { std::free(p); }
void operator delete[](void *p) noexcept { std::free(p); }
void operator delete(void *p, std::size_t) noexcept { std::free(p); }
void operator delete[](void *p, std::size_t) noexcept { std::free(p); }

extern "C" {

// preset: 0 = presetDefault, 1 = presetCheaper. split: spread each spectrum's work over
// the following interval (one interval more output latency, no CPU bursts). Returns null on
// failure.
void *ph_stretch_new(int channels, float sample_rate, int preset, unsigned seed, int split) {
    try {
        Stretch *s = new Stretch(long(seed));
        if (preset == 1) {
            s->presetCheaper(channels, sample_rate, split != 0);
        } else {
            s->presetDefault(channels, sample_rate, split != 0);
        }
        return s;
    } catch (...) {
        return nullptr;
    }
}

void ph_stretch_free(void *h) {
    try {
        delete static_cast<Stretch *>(h);
    } catch (...) {
    }
}

void ph_stretch_reset(void *h) {
    try {
        static_cast<Stretch *>(h)->reset();
    } catch (...) {
    }
}

int ph_stretch_input_latency(const void *h) {
    return static_cast<const Stretch *>(h)->inputLatency();
}

int ph_stretch_output_latency(const void *h) {
    return static_cast<const Stretch *>(h)->outputLatency();
}

int ph_stretch_block_samples(const void *h) {
    return static_cast<const Stretch *>(h)->blockSamples();
}

int ph_stretch_interval_samples(const void *h) {
    return static_cast<const Stretch *>(h)->intervalSamples();
}

void ph_stretch_set_transpose(void *h, float semitones) {
    try {
        static_cast<Stretch *>(h)->setTransposeSemitones(semitones);
    } catch (...) {
    }
}

void ph_stretch_seek(void *h, const float *const *input, int frames, double rate) {
    try {
        static_cast<Stretch *>(h)->seek(input, frames, rate);
    } catch (...) {
    }
}

void ph_stretch_process(void *h, const float *const *input, int in_frames, float *const *output,
                        int out_frames) {
    try {
        static_cast<Stretch *>(h)->process(input, in_frames, output, out_frames);
    } catch (...) {
    }
}

// Allocations made by C++ code on the calling thread so far.
size_t ph_stretch_alloc_count(void) { return g_allocs; }

} // extern "C"
