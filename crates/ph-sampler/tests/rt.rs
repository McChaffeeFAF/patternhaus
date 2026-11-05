//! Real-time contract: `process` (note-ons with round robin and stealing, note-offs with
//! release triggers, keyswitches, controllers, AllNotesOff) never allocates.

use ph_core::*;
use ph_node::*;
use ph_sampler::{HausSampler, SampleSet, ZoneSample};
use std::sync::Arc;

#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn sampler() -> HausSampler {
    let mut inst = HausInstrument::new("rt");
    inst.settings.voice_limit = 4;
    inst.articulations.push(Articulation {
        id: ArticulationId(0),
        name: "a".into(),
        trigger: ArticulationTrigger::Key { key: 24 },
        color: None,
    });
    let mut g = Group::new("g");
    g.id = GroupId(0);
    g.round_robin = RoundRobinMode::Random;
    g.voice_limit = Some(3);
    g.stealing = StealMode::Quietest;
    inst.groups.insert(g.id, g);
    let mut set = SampleSet::default();
    let data = Arc::new(SampleData {
        sample_rate: 44_100.0,
        channels: vec![vec![0.5; 20_000], vec![-0.5; 20_000]],
        loop_range: None,
        root_key: None,
    });
    for i in 0..4u32 {
        let mut z = Zone::new(
            GroupId(0),
            SampleRef::File(format!("{i}.wav")),
            KeyRange::ALL,
            60,
        );
        z.id = ZoneId(i);
        z.round_robin = (i < 3).then_some(i);
        z.trigger = if i == 3 {
            ZoneTrigger::Release
        } else {
            ZoneTrigger::Attack
        };
        z.release_decay_db = 3.0;
        set.samples
            .push(ZoneSample::whole(&format!("{i}.wav"), data.clone()));
        set.index.insert(z.id, i as usize);
        inst.zones.insert(z.id, z);
    }
    HausSampler::new(&inst, set)
}

#[test]
fn process_never_allocates() {
    let mut s = sampler();
    s.activate(&ActivateInfo {
        sample_rate: 48_000.0,
        max_frames: 256,
        quality: Quality::Live,
        seed: 9,
    });
    let mut events = Vec::with_capacity(64);
    let mut l = vec![0.0f32; 256];
    let mut r = vec![0.0f32; 256];
    for block in 0..200u32 {
        events.clear();
        let k = 40 + (block % 40) as u8;
        events.push(Event {
            frame: 0,
            kind: EventKind::NoteOn {
                note_id: block,
                key: if block % 17 == 0 { 24 } else { k },
                velocity: 0.7,
                pan: 0.1,
                fine_pitch: 3.0,
            },
        });
        events.push(Event {
            frame: 10,
            kind: EventKind::Cc {
                controller: 1,
                value: 0.5,
            },
        });
        if block > 0 {
            events.push(Event {
                frame: 100,
                kind: EventKind::NoteOff {
                    note_id: block - 1,
                    key: 40 + ((block - 1) % 40) as u8,
                    velocity: 0.0,
                },
            });
        }
        if block % 50 == 49 {
            events.push(Event {
                frame: 200,
                kind: EventKind::AllNotesOff,
            });
        }
        let ctx = ProcessContext {
            sample_rate: 48_000.0,
            frames: 256,
            song_pos_samples: block as i64 * 256,
            song_pos_ticks: 0.0,
            bpm: 120.0,
            playing: true,
            live: true,
        };
        assert_no_alloc::assert_no_alloc(|| {
            let mut chans: [&mut [f32]; 2] = [&mut l, &mut r];
            let mut ports: [&mut [&mut [f32]]; 1] = [&mut chans];
            let mut io = ProcessIo {
                inputs: &[],
                outputs: &mut ports,
                events: &events,
            };
            s.process(&ctx, &mut io);
        });
    }
    assert!(s.voices().iter().filter(|v| v.counts()).count() <= 4);
}

/// A stream over memory; `starve` makes every read an underrun.
struct Mem {
    data: Vec<f32>,
    starve: bool,
}

impl ph_sampler::StreamSource for Mem {
    fn want(&self, _: usize, _: usize, _: u64, _: bool) {}
    fn release(&self, _: usize) {}
    fn read(&self, _: usize, start: i64, out: [&mut [f32]; 2]) -> bool {
        let [l, r] = out;
        for i in 0..l.len() {
            let f = start + i as i64;
            l[i] = if self.starve || f < 0 {
                0.0
            } else {
                self.data.get(f as usize).copied().unwrap_or(0.0)
            };
            r[i] = l[i];
        }
        !self.starve
    }
}

struct Half(NodeDescriptor);

impl Processor for Half {
    fn descriptor(&self) -> &NodeDescriptor {
        &self.0
    }
    fn activate(&mut self, _: &ActivateInfo) {}
    fn process(&mut self, ctx: &ProcessContext, io: &mut ProcessIo<'_, '_>) -> ProcessStatus {
        for c in 0..2 {
            for i in 0..ctx.frames {
                io.outputs[0][c][i] = io.inputs[0][c][i] * 0.5;
            }
        }
        ProcessStatus::Continue
    }
    fn set_param(&mut self, _: ParamId, _: f64) {}
    fn param(&self, _: ParamId) -> f64 {
        0.0
    }
    fn reset(&mut self) {}
}

fn key_of(block: u32) -> u8 {
    if block.is_multiple_of(3) {
        64 + (block % 30) as u8
    } else {
        30 + (block % 30) as u8
    }
}

/// Every M3/T2 feature, streamed (fed and starved), in both qualities: `process` never
/// allocates, including stealing with fades, legato glides, loops, group chains and
/// underruns.
#[test]
fn every_feature_never_allocates() {
    for (quality, starve) in [
        (Quality::Live, false),
        (Quality::High, false),
        (Quality::Live, true),
    ] {
        let mut inst = HausInstrument::new("rt2");
        inst.settings.voice_limit = 6;
        let mut g = Group::new("pad");
        g.id = GroupId(0);
        g.voice_limit = Some(4);
        g.stealing = StealMode::Quietest;
        g.amp_env.attack = 0.01;
        g.amp_env.release = 0.05;
        g.filter_env = Some(Envelope::default());
        g.filter = Some(Filter {
            kind: FilterKind::LowPass24,
            cutoff: 1500.0,
            resonance: 12.0,
            key_track: 100.0,
            env_depth: 1200.0,
        });
        g.lfos = vec![
            Lfo {
                shape: LfoShape::Random,
                rate: 7.0,
                delay: 0.0,
                fade: 0.1,
            },
            Lfo {
                shape: LfoShape::Triangle,
                rate: 3.0,
                delay: 0.0,
                fade: 0.0,
            },
        ];
        let mut lead = Group::new("lead");
        lead.id = GroupId(1);
        lead.polyphony = Polyphony::Legato;
        lead.portamento = 0.02;
        inst.groups.insert(g.id, g);
        inst.groups.insert(lead.id, lead);
        inst.modulation = vec![
            ModRoute {
                source: ModSource::Lfo { index: 0 },
                target: ModTarget::Cutoff,
                depth: 1200.0,
                curve: ModCurve::Linear,
                group: Some(GroupId(0)),
            },
            ModRoute {
                source: ModSource::Random,
                target: ModTarget::SampleStart,
                depth: 500.0,
                curve: ModCurve::Linear,
                group: None,
            },
            ModRoute {
                source: ModSource::Cc { controller: 1 },
                target: ModTarget::Volume,
                depth: -12.0,
                curve: ModCurve::Convex,
                group: None,
            },
        ];
        let frames = 200_000;
        let tone: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.03).sin() * 0.3).collect();
        let whole = Arc::new(SampleData {
            sample_rate: 44_100.0,
            channels: vec![tone[..20_000].to_vec()],
            loop_range: None,
            root_key: None,
        });
        let head = Arc::new(SampleData {
            sample_rate: 44_100.0,
            channels: vec![tone[..4096].to_vec()],
            loop_range: None,
            root_key: None,
        });
        let mut set = SampleSet::default();
        set.samples.push(ZoneSample::whole("loop.wav", whole));
        set.samples.push(ZoneSample {
            reference: "long.wav".into(),
            head,
            frames: frames as u64,
            complete: false,
            stream: Some(0),
        });
        set.streams = Some(Arc::new(Mem { data: tone, starve }));
        let file = |n: &str| SampleRef::File(n.into());
        let mut looped = Zone::new(GroupId(0), file("loop.wav"), KeyRange { lo: 0, hi: 63 }, 60);
        looped.id = ZoneId(0);
        looped.looping = Some(ZoneLoop {
            mode: LoopMode::Forward,
            start: 5000,
            end: 15_000,
            crossfade: 2000,
            on_release: LoopRelease::Exit,
        });
        looped.crossfades = vec![Crossfade {
            source: CrossfadeSource::Cc { controller: 1 },
            in_lo: 0.0,
            in_hi: 0.5,
            out_lo: 1.0,
            out_hi: 1.0,
            curve: CrossfadeCurve::Power,
        }];
        let mut long = Zone::new(GroupId(0), file("long.wav"), KeyRange { lo: 0, hi: 63 }, 60);
        long.id = ZoneId(1);
        let mut solo = Zone::new(
            GroupId(1),
            file("long.wav"),
            KeyRange { lo: 64, hi: 127 },
            72,
        );
        solo.id = ZoneId(2);
        for (i, z) in [looped, long, solo].into_iter().enumerate() {
            set.index.insert(z.id, i.min(1));
            inst.zones.insert(z.id, z);
        }
        let mut s = HausSampler::new(&inst, set);
        let mut d = ph_sampler::descriptor(0);
        d.id = "test.half".into();
        s.set_group_effects(1, vec![(Box::new(Half(d)), 0.5)]);
        s.activate(&ActivateInfo {
            sample_rate: 48_000.0,
            max_frames: 256,
            quality,
            seed: 3,
        });
        let mut events = Vec::with_capacity(64);
        let mut l = vec![0.0f32; 256];
        let mut r = vec![0.0f32; 256];
        for block in 0..400u32 {
            events.clear();
            events.push(Event {
                frame: 0,
                kind: EventKind::NoteOn {
                    note_id: block,
                    key: key_of(block),
                    velocity: 0.8,
                    pan: 0.0,
                    fine_pitch: 0.0,
                },
            });
            events.push(Event {
                frame: 5,
                kind: EventKind::Cc {
                    controller: 1,
                    value: (block % 10) as f32 / 9.0,
                },
            });
            events.push(Event {
                frame: 9,
                kind: EventKind::PitchBend {
                    value: ((block % 7) as f32 - 3.0) / 3.0,
                },
            });
            events.push(Event {
                frame: 20,
                kind: EventKind::NoteExpression {
                    note_id: block,
                    kind: NoteExpressionKind::Pressure,
                    value: 0.5,
                },
            });
            if block > 2 {
                events.push(Event {
                    frame: 100,
                    kind: EventKind::NoteOff {
                        note_id: block - 3,
                        key: key_of(block - 3),
                        velocity: 0.0,
                    },
                });
            }
            if block % 150 == 149 {
                events.push(Event {
                    frame: 200,
                    kind: EventKind::AllNotesOff,
                });
            }
            let ctx = ProcessContext {
                sample_rate: 48_000.0,
                frames: 256,
                song_pos_samples: block as i64 * 256,
                song_pos_ticks: 0.0,
                bpm: 120.0,
                playing: true,
                live: true,
            };
            assert_no_alloc::assert_no_alloc(|| {
                let mut chans: [&mut [f32]; 2] = [&mut l, &mut r];
                let mut ports: [&mut [&mut [f32]]; 1] = [&mut chans];
                let mut io = ProcessIo {
                    inputs: &[],
                    outputs: &mut ports,
                    events: &events,
                };
                s.process(&ctx, &mut io);
            });
        }
        assert!(s.voices().iter().filter(|v| v.counts()).count() <= 6);
    }
}
