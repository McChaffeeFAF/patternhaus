//! Instrument test helpers: synthetic samples and a map-backed asset resolver.
#![allow(dead_code)]

use ph_node::{AssetResolver, Processor, SampleData};
use std::collections::HashMap;
use std::sync::Arc;

pub struct MapResolver(pub HashMap<String, Arc<SampleData>>);

impl AssetResolver for MapResolver {
    fn sample(&self, reference: &str) -> Result<Arc<SampleData>, String> {
        self.0
            .get(reference)
            .cloned()
            .ok_or_else(|| format!("no sample `{reference}`"))
    }
}

/// Stereo, 44.1 kHz, 0.5 s: a decaying 220 Hz saw-ish tone plus noise, with a file loop.
pub fn test_sample() -> Arc<SampleData> {
    let n = 22050;
    let mut r = ph_dsp::Rng::new(17);
    let ch = |k: f32, r: &mut ph_dsp::Rng| -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 44100.0;
                let ph = (220.0 * t * k).fract();
                (0.6 * (2.0 * ph - 1.0) + 0.1 * r.bipolar()) * (-3.0 * t).exp()
            })
            .collect()
    };
    let l = ch(1.0, &mut r);
    let rr = ch(1.003, &mut r);
    Arc::new(SampleData {
        sample_rate: 44100.0,
        channels: vec![l, rr],
        loop_range: Some((4410, 17640)),
        root_key: Some(57),
    })
}

/// Mono sine sample.
pub fn sine_sample(freq: f64, sr: f64, seconds: f64) -> Arc<SampleData> {
    let n = (sr * seconds) as usize;
    Arc::new(SampleData {
        sample_rate: sr,
        channels: vec![
            (0..n)
                .map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / sr).sin() as f32 * 0.5)
                .collect(),
        ],
        loop_range: None,
        root_key: None,
    })
}

pub fn set(p: &mut dyn Processor, params: &[(&str, f64)]) {
    for (k, v) in params {
        let id = p
            .descriptor()
            .param_id(k)
            .unwrap_or_else(|| panic!("no param {k}"));
        p.set_param(id, *v);
    }
}

pub fn synth_with(params: &[(&str, f64)]) -> Box<dyn Processor> {
    let mut p: Box<dyn Processor> = Box::new(ph_instruments::synth::Synth::new());
    set(p.as_mut(), params);
    p
}

pub fn sampler_with(sample: Arc<SampleData>, params: &[(&str, f64)]) -> Box<dyn Processor> {
    let mut p: Box<dyn Processor> = Box::new(ph_instruments::sampler::Sampler::new(
        Some(sample),
        Some("test:sample".into()),
    ));
    set(p.as_mut(), params);
    p
}
