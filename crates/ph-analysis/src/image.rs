//! PNG snapshots for vision models: waveform, spectrogram, piano roll and playlist overview.
//!
//! Every picture is self-explaining: a title line, labelled axes (bar numbers on top, seconds
//! below, note names or Hz on the side), a legend where colours mean something, and large
//! 2x text. Rendered with `tiny-skia`; the font is built in (see `text`).

use crate::music::{SongNote, note_name};
use crate::text::{self, draw, draw_centered, draw_right, fill, height};
use crate::{Timing, mono_mix};
use ph_dsp::analysis::fft;
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

const BG: [u8; 3] = [0x14, 0x16, 0x1a];
const PANEL: [u8; 3] = [0x1b, 0x1e, 0x24];
const GRID: [u8; 3] = [0x2c, 0x31, 0x3a];
const GRID_STRONG: [u8; 3] = [0x4a, 0x52, 0x60];
const TEXT: [u8; 3] = [0xe2, 0xe6, 0xee];
const DIM: [u8; 3] = [0x9a, 0xa3, 0xb2];
const SCALE: u32 = 2;

fn col(c: [u8; 3]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], 255)
}

fn rgb(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

struct Canvas {
    pm: Pixmap,
    w: i32,
    h: i32,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        let mut pm = Pixmap::new(w, h).expect("image size");
        pm.fill(col(BG));
        Self {
            pm,
            w: w as i32,
            h: h as i32,
        }
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 3]) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.w), (y + h).min(self.h));
        if x1 > x0 && y1 > y0 {
            fill(
                &mut self.pm,
                x0 as f32,
                y0 as f32,
                (x1 - x0) as f32,
                (y1 - y0) as f32,
                col(c),
            );
        }
    }

    fn px(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let w = self.w as usize;
            if let Some(p) = PremultipliedColorU8::from_rgba(c[0], c[1], c[2], 255) {
                self.pm.pixels_mut()[y as usize * w + x as usize] = p;
            }
        }
    }

    fn text(&mut self, s: &str, x: i32, y: i32, c: [u8; 3]) {
        draw(&mut self.pm, s, x, y, SCALE, col(c));
    }

    fn text_right(&mut self, s: &str, x: i32, y: i32, c: [u8; 3]) {
        draw_right(&mut self.pm, s, x, y, SCALE, col(c));
    }

    fn text_center(&mut self, s: &str, x: i32, y: i32, c: [u8; 3]) {
        draw_center(self, s, x, y, c);
    }

    fn png(&self) -> Vec<u8> {
        self.pm.encode_png().expect("encode png")
    }
}

fn draw_center(c: &mut Canvas, s: &str, x: i32, y: i32, color: [u8; 3]) {
    draw_centered(&mut c.pm, s, x, y, SCALE, col(color));
}

fn th() -> i32 {
    height(SCALE) as i32
}

/// Maps ticks to x pixels and draws the bar ruler.
struct TimeAxis {
    x0: i32,
    x1: i32,
    t0: f64,
    t1: f64,
    timing: Timing,
}

impl TimeAxis {
    fn x(&self, ticks: f64) -> i32 {
        self.x0
            + ((ticks - self.t0) / (self.t1 - self.t0) * (self.x1 - self.x0) as f64).round() as i32
    }

    fn bar_step(&self) -> u64 {
        let per_bar =
            (self.x1 - self.x0) as f64 * self.timing.ticks_per_bar() as f64 / (self.t1 - self.t0);
        let mut step = 1u64;
        while (per_bar * step as f64) < 44.0 {
            step *= 2;
        }
        step
    }

    /// Bar lines from `top` to `bottom`, numbers above `top`. Beat lines when there is room.
    fn grid(&self, c: &mut Canvas, top: i32, bottom: i32, label_y: i32) {
        let tpb = self.timing.ticks_per_bar() as f64;
        let per_bar_px = (self.x1 - self.x0) as f64 * tpb / (self.t1 - self.t0);
        let first_bar = (self.t0 / tpb).floor() as i64;
        let last_bar = (self.t1 / tpb).ceil() as i64;
        if per_bar_px >= 8.0 * self.timing.time_signature.numerator as f64 {
            let tpbeat = self.timing.ticks_per_beat() as f64;
            let mut t = (self.t0 / tpbeat).floor() * tpbeat;
            while t <= self.t1 {
                if t >= self.t0 {
                    c.rect(self.x(t), top, 1, bottom - top, GRID);
                }
                t += tpbeat;
            }
        }
        let step = self.bar_step() as i64;
        for b in first_bar..=last_bar {
            let t = b as f64 * tpb;
            if t < self.t0 - 1.0 || t > self.t1 + 1.0 {
                continue;
            }
            let x = self.x(t);
            c.rect(x, top, 1, bottom - top, GRID_STRONG);
            if b % step == 0 && b < last_bar {
                c.text(&format!("{}", b + 1), x + 4, label_y, TEXT);
            }
        }
    }
}

fn title(c: &mut Canvas, s: &str) {
    c.text(s, 12, 10, TEXT);
}

fn seconds_axis(c: &mut Canvas, x0: i32, x1: i32, y: i32, t0: f64, t1: f64) {
    let span = t1 - t0;
    let mut step = 0.5f64;
    while (x1 - x0) as f64 * step / span < 70.0 {
        step *= 2.0;
    }
    // Round the step to something readable: 1, 2, 5, 10...
    let nice = [
        0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0,
    ];
    let step = nice
        .iter()
        .copied()
        .find(|s| *s >= step * 0.99)
        .unwrap_or(step);
    let mut t = (t0 / step).ceil() * step;
    while t <= t1 + 1e-9 {
        let x = x0 + ((t - t0) / span * (x1 - x0) as f64) as i32;
        c.rect(x, y, 1, 5, DIM);
        let label = if step < 1.0 {
            format!("{t:.1}S")
        } else {
            format!("{t:.0}S")
        };
        c.text_center(&label, x, y + 8, DIM);
        t += step;
    }
}

// -------------------------------------------------------------------------------------
// waveform

pub struct WaveformSpec<'a> {
    pub channels: &'a [Vec<f32>],
    pub sample_rate: u32,
    pub timing: Timing,
    /// Range shown, seconds; `None` for everything.
    pub range_s: Option<(f64, f64)>,
    /// Bar number of time 0 of `channels` (1-based), for labels.
    pub first_bar: u32,
    pub width: u32,
}

pub fn waveform(spec: &WaveformSpec) -> Vec<u8> {
    let lanes = spec.channels.len().clamp(1, 2) as i32;
    let (l, r, top, bottom) = (84, 20, 64, 54);
    let lane_h = 150;
    let w = spec.width.clamp(480, 4000) as i32;
    let h = top + bottom + lane_h * lanes;
    let mut c = Canvas::new(w as u32, h as u32);
    let total =
        spec.channels.iter().map(|c| c.len()).max().unwrap_or(0) as f64 / spec.sample_rate as f64;
    let (t0, t1) = spec.range_s.unwrap_or((0.0, total));
    let t1 = t1.max(t0 + 0.01);
    let (x0, x1) = (l, w - r);
    let tpb = &spec.timing;
    let axis = TimeAxis {
        x0,
        x1,
        t0: tpb.seconds_to_ticks(t0) as f64,
        t1: tpb.seconds_to_ticks(t1) as f64,
        timing: spec.timing,
    };
    title(
        &mut c,
        &format!(
            "WAVEFORM  {t0:.1}-{t1:.1} S  BARS {}-{}  {} BPM",
            spec.first_bar as f64 + t0 / tpb.seconds_per_bar(),
            (spec.first_bar as f64 + t1 / tpb.seconds_per_bar()).ceil(),
            tpb.bpm
        ),
    );
    c.text("BAR", 12, 40, DIM);
    let lane_names = ["L", "R"];
    for lane in 0..lanes {
        let y0 = top + lane * lane_h;
        c.rect(x0, y0, x1 - x0, lane_h - 8, PANEL);
        axis.grid(&mut c, y0, y0 + lane_h - 8, 40);
        let mid = y0 + (lane_h - 8) / 2;
        let half = (lane_h - 8) / 2 - 2;
        c.rect(x0, mid, x1 - x0, 1, GRID_STRONG);
        for (v, label) in [
            (1.0, "+1.0"),
            (0.5, "+0.5"),
            (0.0, "0"),
            (-0.5, "-0.5"),
            (-1.0, "-1.0"),
        ] {
            let y = mid - (v * half as f32) as i32;
            c.rect(x0 - 5, y, 5, 1, DIM);
            c.text_right(label, x0 - 9, y, DIM);
        }
        c.text(lane_names[lane as usize], 12, mid - th() / 2, TEXT);
        if let Some(data) = spec.channels.get(lane as usize) {
            let color = if lane == 0 {
                [0x5a, 0xa9, 0xff]
            } else {
                [0xff, 0x9f, 0x4a]
            };
            let sr = spec.sample_rate as f64;
            for px in 0..(x1 - x0) {
                let a = ((t0 + (t1 - t0) * px as f64 / (x1 - x0) as f64) * sr) as usize;
                let b = (((t0 + (t1 - t0) * (px + 1) as f64 / (x1 - x0) as f64) * sr) as usize + 1)
                    .min(data.len());
                if a >= data.len() {
                    break;
                }
                let (mut lo, mut hi) = (0.0f32, 0.0f32);
                for &s in &data[a..b.max(a + 1).min(data.len())] {
                    lo = lo.min(s);
                    hi = hi.max(s);
                }
                let ylo = mid - (hi.clamp(-1.0, 1.0) * half as f32) as i32;
                let yhi = mid - (lo.clamp(-1.0, 1.0) * half as f32) as i32;
                let clipped = hi >= 0.9999 || lo <= -0.9999;
                c.rect(
                    x0 + px,
                    ylo,
                    1,
                    (yhi - ylo).max(1),
                    if clipped { [0xff, 0x4d, 0x4d] } else { color },
                );
            }
        }
    }
    seconds_axis(&mut c, x0, x1, h - bottom + 6, t0, t1);
    c.text("TIME", 12, h - bottom + 14, DIM);
    c.png()
}

// -------------------------------------------------------------------------------------
// spectrogram

pub struct SpectrogramSpec<'a> {
    pub channels: &'a [Vec<f32>],
    pub sample_rate: u32,
    pub timing: Timing,
    pub range_s: Option<(f64, f64)>,
    pub first_bar: u32,
    pub width: u32,
}

fn inferno(t: f32) -> [u8; 3] {
    const STOPS: [(f32, [u8; 3]); 7] = [
        (0.0, [0, 0, 4]),
        (0.16, [40, 11, 84]),
        (0.33, [101, 21, 110]),
        (0.5, [159, 42, 99]),
        (0.66, [212, 72, 66]),
        (0.83, [245, 125, 21]),
        (1.0, [252, 255, 164]),
    ];
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        if t <= w[1].0 {
            return mix(w[0].1, w[1].1, (t - w[0].0) / (w[1].0 - w[0].0));
        }
    }
    STOPS[6].1
}

pub fn spectrogram(spec: &SpectrogramSpec) -> Vec<u8> {
    let (l, r, top, bottom) = (96, 110, 64, 54);
    let plot_h = 440;
    let w = spec.width.clamp(480, 4000) as i32;
    let h = top + bottom + plot_h;
    let mut c = Canvas::new(w as u32, h as u32);
    let sr = spec.sample_rate as f64;
    let mono = mono_mix(spec.channels);
    let total = mono.len() as f64 / sr;
    let (t0, t1) = spec.range_s.unwrap_or((0.0, total));
    let t1 = t1.max(t0 + 0.05);
    let (x0, x1) = (l, w - r);
    let timing = spec.timing;
    let axis = TimeAxis {
        x0,
        x1,
        t0: timing.seconds_to_ticks(t0) as f64,
        t1: timing.seconds_to_ticks(t1) as f64,
        timing,
    };
    title(
        &mut c,
        &format!("SPECTROGRAM  {t0:.1}-{t1:.1} S  LOG FREQUENCY 30 HZ-20 KHZ  COLOUR = DBFS"),
    );
    c.text("BAR", 12, 40, DIM);

    const N: usize = 2048;
    let win = crate::spectrum::hann(N);
    let wsum: f64 = win.iter().sum();
    let (fmin, fmax) = (30.0f64, 20000.0f64.min(sr / 2.0 - 1.0));
    let pw = (x1 - x0) as usize;
    let ph = plot_h as usize;
    let (db_top, db_bottom) = (-10.0f64, -90.0f64);
    // One FFT per pixel column.
    let mut col_mag = vec![0.0f64; N / 2];
    for px in 0..pw {
        let tc = t0 + (t1 - t0) * (px as f64 + 0.5) / pw as f64;
        let centre = (tc * sr) as i64;
        let mut re = vec![0.0; N];
        let mut im = vec![0.0; N];
        for i in 0..N {
            let idx = centre - (N / 2) as i64 + i as i64;
            if idx >= 0 && (idx as usize) < mono.len() {
                re[i] = mono[idx as usize] as f64 * win[i];
            }
        }
        fft(&mut re, &mut im);
        for k in 0..N / 2 {
            col_mag[k] = 2.0 * (re[k] * re[k] + im[k] * im[k]).sqrt() / wsum;
        }
        for py in 0..ph {
            // y=0 is the top (fmax).
            let f_hi = fmin * (fmax / fmin).powf(1.0 - py as f64 / ph as f64);
            let f_lo = fmin * (fmax / fmin).powf(1.0 - (py + 1) as f64 / ph as f64);
            let k_lo = (f_lo * N as f64 / sr).floor() as usize;
            let k_hi = ((f_hi * N as f64 / sr).ceil() as usize).clamp(k_lo + 1, N / 2 - 1);
            let m = col_mag[k_lo.min(N / 2 - 1)..=k_hi]
                .iter()
                .copied()
                .fold(0.0, f64::max);
            let db = if m > 1e-9 { 20.0 * m.log10() } else { -200.0 };
            let t = ((db - db_bottom) / (db_top - db_bottom)) as f32;
            c.px(x0 + px as i32, top + py as i32, inferno(t));
        }
    }
    axis.grid(&mut c, top, top + plot_h, 40);
    // Frequency axis.
    for f in [
        50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0,
    ] {
        if f < fmin || f > fmax {
            continue;
        }
        let y = top + ((1.0 - (f / fmin).ln() / (fmax / fmin).ln()) * plot_h as f64) as i32;
        c.rect(x1, y, 5, 1, DIM);
        c.rect(x0 - 5, y, 5, 1, DIM);
        let label = if f >= 1000.0 {
            format!("{} KHZ", f / 1000.0)
        } else {
            format!("{f} HZ")
        };
        c.text_right(&label, x0 - 9, y, DIM);
    }
    // Colour bar.
    let (bx, bw) = (x1 + 22, 18);
    for py in 0..plot_h {
        let t = 1.0 - py as f32 / plot_h as f32;
        c.rect(bx, top + py, bw, 1, inferno(t));
    }
    for db in [-10, -30, -50, -70, -90] {
        let y = top + ((db as f64 - db_top) / (db_bottom - db_top) * plot_h as f64) as i32;
        c.rect(bx + bw, y, 4, 1, DIM);
        c.text(&format!("{db}"), bx + bw + 8, y - th() / 2, DIM);
    }
    c.text("DBFS", bx - 2, top - th() - 6, DIM);
    seconds_axis(&mut c, x0, x1, top + plot_h + 6, t0, t1);
    c.text("TIME", 12, top + plot_h + 14, DIM);
    let _ = spec.first_bar;
    c.png()
}

// -------------------------------------------------------------------------------------
// piano roll

pub struct RollNote {
    pub key: u8,
    pub start: u64,
    pub length: u64,
    pub velocity: f32,
    /// Index into `legend`.
    pub group: usize,
}

pub struct PianoRollSpec<'a> {
    pub title: String,
    pub notes: &'a [RollNote],
    /// `(name, 0xRRGGBB)` per group, shown as a legend.
    pub legend: &'a [(String, u32)],
    pub timing: Timing,
    /// Ticks shown.
    pub range: (u64, u64),
    pub width: u32,
}

pub fn piano_roll(spec: &PianoRollSpec) -> Vec<u8> {
    let (l, r, top, bottom) = (96, 20, 100, 24);
    let w = spec.width.clamp(480, 4000) as i32;
    let (lo_key, hi_key) = spec
        .notes
        .iter()
        .fold((127u8, 0u8), |(a, b), n| (a.min(n.key), b.max(n.key)));
    let (lo_key, hi_key) = if spec.notes.is_empty() {
        (48, 72)
    } else {
        (lo_key.saturating_sub(2), (hi_key + 2).min(127))
    };
    // Make at least two octaves tall so the keyboard reads clearly.
    let (lo_key, hi_key) = if hi_key - lo_key < 23 {
        let mid = (lo_key as i32 + hi_key as i32) / 2;
        ((mid - 12).max(0) as u8, ((mid + 12).min(127)) as u8)
    } else {
        (lo_key, hi_key)
    };
    let rows = (hi_key - lo_key + 1) as i32;
    let row_h = (560 / rows).clamp(8, 20);
    let plot_h = rows * row_h;
    let h = top + plot_h + bottom;
    let mut c = Canvas::new(w as u32, h as u32);
    let (x0, x1) = (l, w - r);
    let (t0, t1) = (
        spec.range.0 as f64,
        spec.range.1.max(spec.range.0 + 1) as f64,
    );
    let axis = TimeAxis {
        x0,
        x1,
        t0,
        t1,
        timing: spec.timing,
    };
    title(&mut c, &spec.title);
    // Legend.
    let mut lx = 12;
    for (name, color) in spec.legend {
        c.rect(lx, 40, 14, 14, rgb(*color));
        c.text(name, lx + 20, 40, TEXT);
        lx += 20 + text::width(name, SCALE) as i32 + 24;
        if lx > w - 100 {
            break;
        }
    }
    c.text("BAR", 12, 76, DIM);
    // Rows.
    for key in lo_key..=hi_key {
        let y = top + (hi_key - key) as i32 * row_h;
        let black = matches!(key % 12, 1 | 3 | 6 | 8 | 10);
        c.rect(
            x0,
            y,
            x1 - x0,
            row_h,
            if black { [0x17, 0x19, 0x1e] } else { PANEL },
        );
        c.rect(
            x0,
            y + row_h - 1,
            x1 - x0,
            1,
            if key % 12 == 0 { GRID_STRONG } else { GRID },
        );
        // Keyboard strip.
        c.rect(
            12,
            y,
            x0 - 14,
            row_h - 1,
            if black {
                [0x30, 0x33, 0x3b]
            } else {
                [0xd5, 0xd9, 0xe0]
            },
        );
        if key % 12 == 0 || row_h >= 14 {
            let text_col = if black { TEXT } else { [0x1a, 0x1d, 0x23] };
            // Labels are 14 px tall; skip those that do not fit the row.
            if row_h >= 14 || key % 12 == 0 {
                c.text(&note_name(key), 16, y + (row_h - th()) / 2, text_col);
            }
        }
    }
    axis.grid(&mut c, top, top + plot_h, 76);
    // Notes.
    for n in spec.notes {
        let (s, e) = (n.start as f64, (n.start + n.length) as f64);
        if e <= t0 || s >= t1 || n.key < lo_key || n.key > hi_key {
            continue;
        }
        let xa = axis.x(s.max(t0));
        let xb = axis.x(e.min(t1)).max(xa + 2);
        let y = top + (hi_key - n.key) as i32 * row_h;
        let base = rgb(spec.legend.get(n.group).map_or(0x5f9ea0, |g| g.1));
        let shade = mix(
            [0x20, 0x22, 0x28],
            base,
            0.45 + 0.55 * n.velocity.clamp(0.0, 1.0),
        );
        c.rect(xa, y + 1, xb - xa, row_h - 2, [0, 0, 0]);
        c.rect(
            xa + 1,
            y + 2,
            (xb - xa - 2).max(1),
            (row_h - 4).max(1),
            shade,
        );
    }
    // Bottom axis: bar numbers repeated.
    c.text(
        "BAR NUMBERS ABOVE. ROWS = PITCH (C4 = MIDI 60).",
        x0,
        top + plot_h + 6,
        DIM,
    );
    c.png()
}

/// Piano roll notes from song notes, grouped by channel.
pub fn roll_notes(
    notes: &[SongNote],
    group_of: &dyn Fn(ph_core::ChannelId) -> usize,
) -> Vec<RollNote> {
    notes
        .iter()
        .map(|n| RollNote {
            key: n.key,
            start: n.start,
            length: n.length,
            velocity: n.velocity,
            group: group_of(n.channel),
        })
        .collect()
}

// -------------------------------------------------------------------------------------
// playlist

pub struct PlaylistTrackRow {
    pub name: String,
    pub muted: bool,
}

pub struct PlaylistClipBox {
    pub track: usize,
    pub start: u64,
    pub length: u64,
    pub label: String,
    pub color: u32,
}

pub struct PlaylistSpec<'a> {
    pub title: String,
    pub tracks: &'a [PlaylistTrackRow],
    pub clips: &'a [PlaylistClipBox],
    pub timing: Timing,
    pub range: (u64, u64),
    pub width: u32,
}

pub fn playlist(spec: &PlaylistSpec) -> Vec<u8> {
    let (l, r, top, bottom) = (170, 20, 76, 20);
    let w = spec.width.clamp(480, 4000) as i32;
    let rows = spec.tracks.len().max(1) as i32;
    let row_h = 44;
    let h = top + rows * row_h + bottom;
    let mut c = Canvas::new(w as u32, h as u32);
    let (x0, x1) = (l, w - r);
    let axis = TimeAxis {
        x0,
        x1,
        t0: spec.range.0 as f64,
        t1: spec.range.1.max(spec.range.0 + 1) as f64,
        timing: spec.timing,
    };
    title(&mut c, &spec.title);
    c.text("BAR", 12, 52, DIM);
    for (i, t) in spec.tracks.iter().enumerate() {
        let y = top + i as i32 * row_h;
        c.rect(
            x0,
            y,
            x1 - x0,
            row_h - 2,
            if i % 2 == 0 {
                PANEL
            } else {
                [0x18, 0x1a, 0x20]
            },
        );
        let name = if t.muted {
            format!("{} (MUTED)", t.name)
        } else {
            t.name.clone()
        };
        c.text(
            &name,
            12,
            y + (row_h - th()) / 2,
            if t.muted { DIM } else { TEXT },
        );
    }
    axis.grid(&mut c, top, top + rows * row_h, 52);
    for clip in spec.clips {
        let (s, e) = (clip.start as f64, (clip.start + clip.length) as f64);
        if e <= axis.t0 || s >= axis.t1 || clip.track >= spec.tracks.len() {
            continue;
        }
        let xa = axis.x(s.max(axis.t0));
        let xb = axis.x(e.min(axis.t1));
        let y = top + clip.track as i32 * row_h;
        let base = rgb(clip.color);
        c.rect(xa, y + 3, xb - xa, row_h - 8, [0, 0, 0]);
        c.rect(
            xa + 1,
            y + 4,
            (xb - xa - 2).max(1),
            row_h - 10,
            mix([0x14, 0x16, 0x1a], base, 0.8),
        );
        // Label, trimmed to fit.
        let max_chars = ((xb - xa - 10) / ((text::GLYPH_W + 1) * SCALE) as i32).max(0) as usize;
        let label: String = clip.label.chars().take(max_chars).collect();
        let tc = if luminance(base) > 150.0 {
            [0x10, 0x12, 0x16]
        } else {
            [0xf4, 0xf6, 0xfa]
        };
        c.text(&label, xa + 6, y + (row_h - th()) / 2 - 1, tc);
    }
    c.text(
        "EACH BOX IS A CLIP: LABEL = PATTERN NAME.",
        x0,
        top + rows * row_h + 4,
        DIM,
    );
    c.png()
}

fn luminance(c: [u8; 3]) -> f32 {
    0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::*;
    use ph_core::TimeSignature;

    fn decode(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let mut rd = dec.read_info().unwrap();
        let mut buf = vec![0; rd.output_buffer_size().unwrap()];
        let info = rd.next_frame(&mut buf).unwrap();
        (info.width, info.height, buf)
    }

    fn timing() -> Timing {
        Timing::new(120.0, TimeSignature::default())
    }

    fn distinct_colours(buf: &[u8]) -> usize {
        let mut set = std::collections::HashSet::new();
        for p in buf.chunks(4) {
            set.insert([p[0], p[1], p[2]]);
        }
        set.len()
    }

    #[test]
    fn waveform_is_a_valid_png_with_content() {
        let x = sine(220.0, 0.6, 48_000, 4.0);
        let png = waveform(&WaveformSpec {
            channels: &[x.clone(), x],
            sample_rate: 48_000,
            timing: timing(),
            range_s: None,
            first_bar: 1,
            width: 1000,
        });
        let (w, h, buf) = decode(&png);
        assert_eq!(w, 1000);
        assert!(h > 300);
        assert!(distinct_colours(&buf) > 6);
    }

    #[test]
    fn spectrogram_shows_a_tone_at_the_right_height() {
        let x = sine(1000.0, 0.5, 48_000, 2.0);
        let png = spectrogram(&SpectrogramSpec {
            channels: &[x],
            sample_rate: 48_000,
            timing: timing(),
            range_s: None,
            first_bar: 1,
            width: 800,
        });
        let (w, _h, buf) = decode(&png);
        // Plot area: top=64; 1 kHz sits at ln(1000/30)/ln(20000/30) = 0.5386 from the bottom.
        let plot_h = 440.0;
        let y = (64.0 + (1.0 - 0.5386) * plot_h) as usize;
        let x = 96 + 250;
        let at = |yy: usize| {
            let i = (yy * w as usize + x) * 4;
            buf[i] as u32 + buf[i + 1] as u32 + buf[i + 2] as u32
        };
        assert!(at(y) > 400, "tone row should be bright: {}", at(y));
        assert!(
            at(y - 80) < 150,
            "other rows should be dark: {}",
            at(y - 80)
        );
    }

    #[test]
    fn piano_roll_and_playlist_render() {
        let notes: Vec<RollNote> = (0..16)
            .map(|i| RollNote {
                key: 48 + (i % 8) as u8,
                start: i as u64 * 960,
                length: 900,
                velocity: 0.8,
                group: (i % 2) as usize,
            })
            .collect();
        let legend = vec![
            ("BASS".to_string(), 0xd2691e),
            ("KEYS".to_string(), 0x4682b4),
        ];
        let png = piano_roll(&PianoRollSpec {
            title: "PIANO ROLL".into(),
            notes: &notes,
            legend: &legend,
            timing: timing(),
            range: (0, 4 * 3840),
            width: 1200,
        });
        let (w, h, buf) = decode(&png);
        assert_eq!(w, 1200);
        assert!(h > 300 && distinct_colours(&buf) > 8);

        let tracks = vec![
            PlaylistTrackRow {
                name: "TRACK 1".into(),
                muted: false,
            },
            PlaylistTrackRow {
                name: "TRACK 2".into(),
                muted: true,
            },
        ];
        let clips = vec![PlaylistClipBox {
            track: 0,
            start: 0,
            length: 7680,
            label: "BEAT".into(),
            color: 0x5f9ea0,
        }];
        let png = playlist(&PlaylistSpec {
            title: "PLAYLIST".into(),
            tracks: &tracks,
            clips: &clips,
            timing: timing(),
            range: (0, 8 * 3840),
            width: 1200,
        });
        let (w, _, buf) = decode(&png);
        assert_eq!(w, 1200);
        assert!(distinct_colours(&buf) > 8);
    }
}
