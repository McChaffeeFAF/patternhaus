//! A scripted agent for demos and tests: edits the project live through the same session the
//! human uses, as `agent:demo`, so co-editing (flashes, badges, per-author undo, conflicts) can
//! be seen and tested without a real MCP client.

use ph_commands::{Caller, Grant, Request, Scope, SessionHandle};
use ph_core::Project;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const NAME: &str = "demo";

pub fn caller() -> Caller {
    Caller::agent(NAME, Grant(vec![Scope::Compose, Scope::Mix]))
}

/// The first pattern and a channel to write to (the first channel in the rack).
pub fn target(p: &Project) -> Option<(u32, u32)> {
    let pat = p.pattern_order.first()?;
    let ch = p.channel_order.first()?;
    Some((pat.0, ch.0))
}

/// One scripted edit, `k`-th in the sequence: a note that walks up a minor arpeggio, with a
/// `ui.select` pointing at it. Every fifth step nudges insert 1's volume instead. Returns the
/// command ids it ran.
pub fn step(session: &SessionHandle, k: u32) -> Vec<String> {
    let Ok(p) = session.project() else {
        return Vec::new();
    };
    let Some((pat, ch)) = target(&p) else {
        return Vec::new();
    };
    let who = caller();
    let mut ran = Vec::new();
    if k % 5 == 4 {
        let vol = 0.6 + (k % 3) as f64 * 0.1;
        if session
            .execute(
                &who,
                Request::new("mixer.set_volume", json!({"insert": 1, "volume": vol})),
            )
            .is_ok()
        {
            ran.push("mixer.set_volume".to_string());
        }
        return ran;
    }
    let arp = [0u8, 3, 7, 12, 15, 19, 12, 7];
    let key = 48 + arp[(k as usize) % arp.len()];
    let start = (k as u64 % 16) * 240;
    let added = session.execute(
        &who,
        Request::new(
            "note.add",
            json!({"pattern": pat, "channel": ch, "notes": [
                {"key": key, "start": start, "length": 240, "velocity": 0.7}
            ]}),
        ),
    );
    if let Ok(ex) = added {
        ran.push("note.add".to_string());
        if let Some(id) = ex.result["notes"][0].as_u64() {
            let path = format!("note/{pat}/{ch}/{id}");
            if session
                .execute(&who, Request::new("ui.select", json!({"paths": [path]})))
                .is_ok()
            {
                ran.push("ui.select".to_string());
            }
        }
    }
    ran
}

/// Runs [`step`] every `interval` until `stop` is set.
pub fn run(session: SessionHandle, stop: Arc<AtomicBool>, interval: Duration) {
    let mut k = 0;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(interval);
        step(&session, k);
        k += 1;
    }
}
