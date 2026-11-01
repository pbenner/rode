//! Graceful disconnect drains bursts off the GUI thread before scoped teardown.
use crate::process::{self, Session};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Move graceful DAP disconnect to a worker, drain output in batches and then
/// request scoped transport teardown. Clear `closing` only after this sequence finishes;
/// `terminate` controls whether the debuggee is ended rather than detached.
pub(super) fn start(session: Session, sequence: u64, terminate: bool, closing: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let disconnect = json!({
            "op":"dap",
            "message":{
                "seq":sequence,
                "type":"request",
                "command":"disconnect",
                "arguments":{"terminateDebuggee":terminate,"suspendDebuggee":false}
            }
        });
        let mut request = process::encode_request(&disconnect).expect("bounded disconnect request");
        let writer = session.writer();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut sent = false;
        while Instant::now() < deadline {
            if !sent {
                sent = writer.try_send_buffer(&mut request).unwrap_or(false);
            }
            let mut received = false;
            let mut replied = false;
            for _ in 0..256 {
                let Some(event) = session.poll() else {
                    break;
                };
                received = true;
                if matches!(&event,process::Event::Message(v) if v["kind"]=="dap" && v["message"]["type"]=="response" && v["message"]["request_seq"]==sequence)
                {
                    replied = true;
                    break;
                }
            }
            if replied || session.is_finished() {
                break;
            }
            if !received {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = writer.send(&json!({"op":"shutdown"}));
        if !wait_finished(&session, Duration::from_secs(2)) {
            session.stop();
            wait_finished(&session, Duration::from_millis(1800));
        }
        closing.store(false, Ordering::Release);
    });
}
/// Drain pending transport events while waiting at most the supplied duration for supervision to finish.
fn wait_finished(session: &Session, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        // Drain in batches: a fixed delay per message would make a verbose
        // target look unresponsive even when it had already disconnected.
        let mut received = false;
        for _ in 0..256 {
            if session.poll().is_none() {
                break;
            }
            received = true;
        }
        if session.is_finished() {
            return true;
        }
        if !received {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    false
}
