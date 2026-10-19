//! The audio editor's operations (M2/T6), as pure functions over de-interleaved samples.
//!
//! Every operation returns a **new** buffer; nothing here touches a file. `editor.apply`
//! writes the result to a new file in the project's `samples/` and repoints the project at
//! it, so the original is never changed and undo is the ordinary patch undo.
//!
//! A *range* is `start..end` in frames, clamped to the buffer.

use ph_core::FadeCurve;

/// De-interleaved audio: one `Vec` per channel, all the same length.
pub type Channels = Vec<Vec<f32>>;

/// Frames per channel.
pub fn frames(c: &[Vec<f32>]) -> usize {
    c.first().map_or(0, Vec::len)
}

/// `start..end` clamped to `0..len`, with `start <= end`.
pub fn clamp_range(len: usize, start: Option<u64>, end: Option<u64>) -> (usize, usize) {
    let e = end.map_or(len, |e| (e as usize).min(len));
    let s = start.map_or(0, |s| (s as usize).min(e));
    (s, e)
}

/// Keeps only `range`.
pub fn trim(c: &[Vec<f32>], (s, e): (usize, usize)) -> Channels {
    c.iter().map(|ch| ch[s..e].to_vec()).collect()
}

/// Removes `range`.
pub fn cut(c: &[Vec<f32>], (s, e): (usize, usize)) -> Channels {
    c.iter()
        .map(|ch| {
            let mut v = ch[..s].to_vec();
            v.extend_from_slice(&ch[e..]);
            v
        })
        .collect()
}

/// Inserts `data` at frame `at` (longer material pushes the rest later).
pub fn insert(c: &[Vec<f32>], data: &[Vec<f32>], at: usize) -> Channels {
    c.iter()
        .enumerate()
        .map(|(i, ch)| {
            let at = at.min(ch.len());
            let src = &data[i.min(data.len() - 1)];
            let mut v = ch[..at].to_vec();
            v.extend_from_slice(src);
            v.extend_from_slice(&ch[at..]);
            v
        })
        .collect()
}

/// Sets `range` to silence.
pub fn silence(c: &[Vec<f32>], (s, e): (usize, usize)) -> Channels {
    let mut out = c.to_vec();
    for ch in &mut out {
        ch[s..e].fill(0.0);
    }
    out
}

/// Reverses `range` in place.
pub fn reverse(c: &[Vec<f32>], (s, e): (usize, usize)) -> Channels {
    let mut out = c.to_vec();
    for ch in &mut out {
        ch[s..e].reverse();
    }
    out
}

/// Multiplies `range` by `gain` (linear).
pub fn gain(c: &[Vec<f32>], (s, e): (usize, usize), gain: f64) -> Channels {
    let g = gain as f32;
    let mut out = c.to_vec();
    for ch in &mut out {
        for x in &mut ch[s..e] {
            *x *= g;
        }
    }
    out
}

/// Peak of `range` over all channels (linear).
pub fn peak(c: &[Vec<f32>], (s, e): (usize, usize)) -> f32 {
    c.iter()
        .flat_map(|ch| ch[s..e].iter())
        .fold(0.0f32, |m, x| m.max(x.abs()))
}

/// Scales `range` so its peak reaches `target_db` dBFS. `None` when the range is silent.
pub fn normalize(c: &[Vec<f32>], range: (usize, usize), target_db: f64) -> Option<Channels> {
    let p = peak(c, range);
    if p <= 1e-9 {
        return None;
    }
    let target = 10f64.powf(target_db / 20.0);
    Some(gain(c, range, target / p as f64))
}

/// A fade over `range`: from silence up to full level (`fade_in`) or from full level down to
/// silence. The curve is the same one audio clips use.
pub fn fade(c: &[Vec<f32>], (s, e): (usize, usize), curve: FadeCurve, fade_in: bool) -> Channels {
    let n = e - s;
    let mut out = c.to_vec();
    for ch in &mut out {
        for (i, x) in ch[s..e].iter_mut().enumerate() {
            // frame centres, so a one-frame fade is not a hard mute or a no-op
            let t = (i as f64 + 0.5) / n as f64;
            let g = if fade_in {
                curve.gain(t)
            } else {
                curve.gain(1.0 - t)
            };
            *x *= g as f32;
        }
    }
    out
}

/// Half-width of the de-click window: clicks up to `2 * HALF` frames long are found.
const HALF: usize = 4;

/// A simple de-click: a frame that is far from the median of its neighbours (more than six
/// times the local deviation, and more than a fixed floor) is replaced by that median.
/// Smooth material is left exactly as it was. Returns the result and how many frames changed.
pub fn declick(c: &[Vec<f32>], (s, e): (usize, usize)) -> (Channels, usize) {
    let mut out = c.to_vec();
    let mut changed = 0;
    for (ch, o) in c.iter().zip(out.iter_mut()) {
        let mut window = [0.0f32; 2 * HALF + 1];
        let mut dev = [0.0f32; 2 * HALF + 1];
        for i in s..e {
            for (k, w) in window.iter_mut().enumerate() {
                let j = (i + k).saturating_sub(HALF).min(ch.len() - 1);
                *w = ch[j];
            }
            let med = median(&mut window.clone());
            for (d, w) in dev.iter_mut().zip(window.iter()) {
                *d = (w - med).abs();
            }
            let mad = median(&mut dev.clone());
            let off = (ch[i] - med).abs();
            if off > 6.0 * mad.max(1e-4) && off > 0.02 {
                o[i] = med;
                changed += 1;
            }
        }
    }
    (out, changed)
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Brings `data` to `channels` channels (mono is copied to both sides of a stereo target; a
/// stereo source is averaged into a mono one).
pub fn match_channels(data: &[Vec<f32>], channels: usize) -> Channels {
    match (data.len(), channels) {
        (a, b) if a == b => data.to_vec(),
        (1, b) => vec![data[0].clone(); b],
        (_, 1) => {
            let n = frames(data);
            vec![
                (0..n)
                    .map(|i| data.iter().map(|ch| ch[i]).sum::<f32>() / data.len() as f32)
                    .collect(),
            ]
        }
        (_, b) => (0..b)
            .map(|i| data[i.min(data.len() - 1)].clone())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Channels {
        vec![(0..n).map(|i| i as f32).collect(), vec![1.0; n]]
    }

    #[test]
    fn trim_cut_and_insert_are_inverse_shaped() {
        let c = ramp(10);
        assert_eq!(trim(&c, (2, 5))[0], vec![2.0, 3.0, 4.0]);
        assert_eq!(
            cut(&c, (2, 8))[0],
            vec![0.0, 1.0, 8.0, 9.0],
            "cut removes the range"
        );
        let ins = insert(&c, &trim(&c, (2, 4)), 6);
        assert_eq!(frames(&ins), 12);
        assert_eq!(ins[0][6..8], [2.0, 3.0]);
        assert_eq!(ins[0][8], 6.0);
    }

    #[test]
    fn range_is_clamped() {
        assert_eq!(clamp_range(10, None, None), (0, 10));
        assert_eq!(clamp_range(10, Some(4), Some(99)), (4, 10));
        assert_eq!(clamp_range(10, Some(8), Some(3)), (3, 3));
    }

    #[test]
    fn normalize_reaches_the_target_peak() {
        let c = vec![vec![0.0, 0.25, -0.5, 0.1]];
        let n = normalize(&c, (0, 4), -6.0).unwrap();
        assert!((peak(&n, (0, 4)) - 0.501_187).abs() < 1e-5);
        assert!(normalize(&[vec![0.0; 4]], (0, 4), 0.0).is_none());
    }

    #[test]
    fn fades_are_exact_at_the_ends_and_monotonic() {
        let c = vec![vec![1.0; 1000]];
        for curve in [
            FadeCurve::Linear,
            FadeCurve::Exp,
            FadeCurve::Log,
            FadeCurve::SCurve,
        ] {
            let f = fade(&c, (0, 1000), curve, true);
            assert!(f[0][0] < 0.01 && f[0][999] > 0.99, "{curve:?}");
            assert!(f[0].windows(2).all(|w| w[1] >= w[0]), "{curve:?}");
            let o = fade(&c, (0, 1000), curve, false);
            assert!(o[0][0] > 0.99 && o[0][999] < 0.01, "{curve:?}");
        }
        // outside the selection nothing changes
        let part = fade(&c, (100, 200), FadeCurve::Linear, true);
        assert_eq!(part[0][99], 1.0);
        assert_eq!(part[0][200], 1.0);
    }

    #[test]
    fn declick_removes_a_spike_and_leaves_a_sine_alone() {
        let sine: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut clicked = sine.clone();
        clicked[900] = 0.95;
        clicked[901] = -0.9;
        clicked[1500] += 0.6;
        let (fixed, n) = declick(&[clicked], (0, 2000));
        assert_eq!(n, 3);
        for (a, b) in fixed[0].iter().zip(&sine) {
            assert!((a - b).abs() < 0.05, "{a} vs {b}");
        }
        let (same, n) = declick(std::slice::from_ref(&sine), (0, 2000));
        assert_eq!(n, 0);
        assert_eq!(same[0], sine, "clean material is untouched");
    }

    #[test]
    fn channels_are_matched_for_paste() {
        let mono = vec![vec![1.0, 2.0]];
        assert_eq!(match_channels(&mono, 2).len(), 2);
        let st = vec![vec![1.0, 1.0], vec![3.0, 3.0]];
        assert_eq!(match_channels(&st, 1)[0], vec![2.0, 2.0]);
    }

    #[test]
    fn reverse_and_silence_touch_only_the_range() {
        let c = ramp(6);
        assert_eq!(reverse(&c, (1, 4))[0], vec![0.0, 3.0, 2.0, 1.0, 4.0, 5.0]);
        assert_eq!(silence(&c, (1, 3))[0], vec![0.0, 0.0, 0.0, 3.0, 4.0, 5.0]);
        assert_eq!(gain(&c, (0, 2), 2.0)[1][..3], [2.0, 2.0, 1.0]);
    }
}
