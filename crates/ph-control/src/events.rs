//! Live events (DESIGN §5): edits by any author, transport changes and overloads, fanned out
//! to every subscriber (MCP connections, JSON-RPC clients, the UI) and kept in a short ring
//! so a client that missed a notification can ask "what happened since #N".

use crate::diff::diff;
use ph_commands::SessionHandle;
use ph_engine::Engine;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    /// Monotonic sequence number.
    pub seq: u64,
    /// `edit`, `project_replaced`, `transport`, `overload`, `warnings` or `plugin_fault`.
    pub kind: &'static str,
    /// `human` or `agent:<name>` for edits.
    pub author: Option<String>,
    pub txn: Option<u64>,
    /// One line, readable.
    pub summary: String,
    pub data: Value,
}

const RING: usize = 256;

#[derive(Default)]
struct Inner {
    next: u64,
    ring: VecDeque<Event>,
    subs: Vec<UnboundedSender<Event>>,
}

pub struct EventHub {
    inner: Mutex<Inner>,
}

impl EventHub {
    /// A hub with no sources, for tests and for front ends that publish by hand.
    pub fn detached() -> Arc<EventHub> {
        Arc::new(EventHub {
            inner: Mutex::new(Inner {
                next: 1,
                ..Default::default()
            }),
        })
    }

    /// Starts the threads that watch the session and the engine.
    pub fn start(session: &SessionHandle, engine: Arc<Engine>) -> Result<Arc<EventHub>, String> {
        let hub = EventHub::detached();
        let changes = session.subscribe().map_err(|e| e.to_string())?;
        let mut prev = session.project().map_err(|e| e.to_string())?;
        let h = hub.clone();
        std::thread::Builder::new()
            .name("ph-events".into())
            .spawn(move || {
                for change in changes {
                    let d = diff(&prev, &change.project);
                    prev = change.project.clone();
                    if change.txn == 0 {
                        h.publish(
                            "project_replaced",
                            None,
                            None,
                            "the project was replaced".into(),
                            json!({}),
                        );
                        continue;
                    }
                    let summary = change
                        .summary
                        .clone()
                        .unwrap_or_else(|| "edited the project".into());
                    h.publish(
                        "edit",
                        Some(change.author.to_string()),
                        Some(change.txn),
                        summary,
                        json!({ "changes": d.lines, "objects": change.paths.len() }),
                    );
                }
            })
            .map_err(|e| e.to_string())?;

        let weak: Weak<EventHub> = Arc::downgrade(&hub);
        std::thread::Builder::new()
            .name("ph-events-poll".into())
            .spawn(move || {
                let (mut playing, mut overloads, mut warned) =
                    (engine.status().playing, engine.stats().overloads, 0usize);
                let mut faults = engine.plugin_faults().len();
                loop {
                    std::thread::sleep(Duration::from_millis(200));
                    let Some(h) = weak.upgrade() else { return };
                    let st = engine.status();
                    if st.playing != playing {
                        playing = st.playing;
                        h.publish(
                            "transport",
                            None,
                            None,
                            if playing {
                                "playback started"
                            } else {
                                "playback stopped"
                            }
                            .into(),
                            json!({ "playing": playing, "position": st.position }),
                        );
                    }
                    let stats = engine.stats();
                    if stats.overloads > overloads {
                        h.publish(
                            "overload",
                            None,
                            None,
                            format!(
                                "audio overload: {} missed deadline(s), DSP load {:.0}%",
                                stats.overloads - overloads,
                                stats.dsp_load * 100.0
                            ),
                            json!({
                                "overloads": stats.overloads,
                                "dsp_load": stats.dsp_load,
                                "suggest_buffer_frames": stats.suggest_buffer_frames
                            }),
                        );
                    }
                    overloads = stats.overloads;
                    // a plugin crashed: it plays silence and is isolated (M2/T3)
                    let all = engine.plugin_faults();
                    for f in all.iter().skip(faults) {
                        h.publish(
                            "plugin_fault",
                            None,
                            None,
                            f.message(),
                            json!({
                                "plugin": f.plugin.id,
                                "name": f.plugin.name,
                                "where": f.slot.to_string(),
                                "call": format!("{:?}", f.fault.call),
                                "message": f.fault.message,
                            }),
                        );
                    }
                    faults = all.len();
                    if stats.warnings.len() != warned {
                        warned = stats.warnings.len();
                        if warned > 0 {
                            h.publish(
                                "warnings",
                                None,
                                None,
                                format!("{warned} engine warning(s)"),
                                json!({ "warnings": stats.warnings }),
                            );
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(hub)
    }

    pub fn publish(
        &self,
        kind: &'static str,
        author: Option<String>,
        txn: Option<u64>,
        summary: String,
        data: Value,
    ) {
        let mut g = self.inner.lock().unwrap();
        let ev = Event {
            seq: g.next,
            kind,
            author,
            txn,
            summary,
            data,
        };
        g.next += 1;
        if g.ring.len() >= RING {
            g.ring.pop_front();
        }
        g.ring.push_back(ev.clone());
        g.subs.retain(|s| s.send(ev.clone()).is_ok());
    }

    /// New events as they happen.
    pub fn subscribe(&self) -> UnboundedReceiver<Event> {
        let (tx, rx) = unbounded_channel();
        self.inner.lock().unwrap().subs.push(tx);
        rx
    }

    /// Events with `seq > after`, oldest first.
    pub fn since(&self, after: u64) -> Vec<Event> {
        self.inner
            .lock()
            .unwrap()
            .ring
            .iter()
            .filter(|e| e.seq > after)
            .cloned()
            .collect()
    }

    pub fn last_seq(&self) -> u64 {
        self.inner.lock().unwrap().next - 1
    }
}
