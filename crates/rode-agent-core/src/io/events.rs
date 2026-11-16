//! Event backpressure follows retained bytes as well as item count. A large
//! reply may occupy the budget alone; diagnostic chunks and small deltas retain
//! the normal burst capacity. Dropping the consumer wakes every blocked reader.
use crate::Event;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};

const ITEMS: usize = 16;
const BYTES: usize = crate::LINE_LIMIT;

#[derive(Default)]
struct State {
    events: VecDeque<(Event, usize)>,
    bytes: usize,
    closed: bool,
}

#[derive(Default)]
struct Queue {
    state: Mutex<State>,
    ready: Condvar,
}

/// Producer for bounded process events. Sending blocks only an I/O worker.
#[derive(Clone)]
pub struct EventSender(Arc<Queue>);

/// Single nonblocking consumer; drop releases workers even under backpressure.
pub struct EventReceiver(Arc<Queue>);

/// Create a queue holding at most sixteen small events or one reply larger than
/// the ordinary 4 MiB budget. This budget is separate from protocol line limits.
pub fn event_channel() -> (EventSender, EventReceiver) {
    let queue = Arc::new(Queue::default());
    (EventSender(queue.clone()), EventReceiver(queue))
}

impl EventSender {
    /// Deliver an event, or return false when its consumer has closed.
    pub fn send(&self, event: Event) -> bool {
        let size = event_size(&event);
        let mut state = self.0.state.lock().unwrap();
        while !state.closed
            && (state.events.len() >= ITEMS
                || !state.events.is_empty() && state.bytes.saturating_add(size) > BYTES)
        {
            state = self.0.ready.wait(state).unwrap();
        }
        if state.closed {
            return false;
        }
        state.bytes = state.bytes.saturating_add(size);
        state.events.push_back((event, size));
        true
    }
}

impl EventReceiver {
    /// Take one event without blocking the controller or UI.
    pub fn poll(&self) -> Option<Event> {
        let mut state = self.0.state.lock().unwrap();
        let (event, size) = state.events.pop_front()?;
        state.bytes -= size;
        self.0.ready.notify_all();
        Some(event)
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.closed = true;
        state.events.clear();
        state.bytes = 0;
        self.0.ready.notify_all();
    }
}

fn event_size(event: &Event) -> usize {
    std::mem::size_of::<Event>()
        + match event {
            Event::Message(value) => value_size(value),
            Event::Stderr(text) | Event::Error(text) => text.capacity(),
            _ => 0,
        }
}

fn value_size(value: &Value) -> usize {
    let extra = match value {
        Value::String(text) => text.capacity(),
        Value::Array(values) => values.iter().map(value_size).sum(),
        Value::Object(values) => values
            .iter()
            // Include map-node overhead rather than counting only JSON text.
            .map(|(key, value)| key.capacity() + 64 + value_size(value))
            .sum(),
        _ => 0,
    };
    std::mem::size_of::<Value>() + extra
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn small_event_bursts_keep_order_and_closing_releases_blocked_workers() {
        let (sender, receiver) = event_channel();
        for n in 0..ITEMS {
            assert!(sender.send(Event::Stderr(n.to_string())));
        }
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(sender.send(Event::Started)).unwrap();
        });
        assert_eq!(receiver.0.state.lock().unwrap().events.len(), ITEMS);
        for n in 0..ITEMS {
            assert!(matches!(receiver.poll(), Some(Event::Stderr(text)) if text == n.to_string()));
        }
        assert!(result.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        assert!(matches!(receiver.poll(), Some(Event::Started)));

        let (sender, receiver) = event_channel();
        for _ in 0..ITEMS {
            assert!(sender.send(Event::Started));
        }
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(sender.send(Event::Started)).unwrap();
        });
        drop(receiver);
        assert!(!result.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn a_large_reply_occupies_the_queue_alone_and_releases_its_budget_when_consumed() {
        let (sender, receiver) = event_channel();
        assert!(sender.send(Event::Message(Value::String("x".repeat(BYTES + 1)))));
        let state = receiver.0.state.lock().unwrap();
        assert!(state.bytes > BYTES);
        assert_eq!(state.events.len(), 1);
        drop(state);
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(sender.send(Event::Started)).unwrap();
        });
        assert!(receiver.poll().is_some());
        assert!(result.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        assert!(matches!(receiver.poll(), Some(Event::Started)));
        let state = receiver.0.state.lock().unwrap();
        assert_eq!(state.bytes, 0);
        assert!(state.events.is_empty());
    }
}
