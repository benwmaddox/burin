//! Live updates: what the writer publishes to the pages people have open (DESIGN.md: live
//! updates). A page holds one stream (`/_burin/live`, server-sent events, which Turbo's
//! `<turbo-stream-source>` reads); each event is a Turbo Stream (`append`, `replace`, `remove`,
//! `refresh`) on a topic (a screen list's instance, a viewer's signals), built once by the writer
//! and sent as the same bytes to every connection on its topic.
//!
//! Events are numbered: a snapshot records the last number it includes, and a page carries it,
//! so a stream that connects after the page was built gets what it missed from the topic's ring
//! (and a reconnect what it missed since its Last-Event-ID). When the ring no longer goes back
//! that far, the stream gets `refresh` (Turbo reloads the page). An event a connection already
//! has (replayed, then also queued) is sent once.
//!
//! The workers own the connections: the writer puts each event in every worker's inbox and wakes
//! it; a worker sends it to its own connections on the topic.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Events kept per topic for replay.
const RING: usize = 256;

/// A topic: a screen list's instance (list index, key).
pub fn list_topic(list: usize, key: i64) -> u64 {
    ((list as u64) << 56) ^ (key as u64 & 0x00ff_ffff_ffff_ffff) ^ 0x1000_0000_0000_0000
}

/// The actions' signals (`signal`): one topic every stream is on, each event for its audience
/// only. A signal is one event however many it's for (no write fan-out); each worker checks its
/// own streams' viewers.
pub const SIGNALS: u64 = 0x2000_0000_0000_0000;

/// One event, as sent: `id: N\nevent: message\ndata: …\n\n`.
#[derive(Clone)]
pub struct Event {
    pub id: u64,
    pub topic: u64,
    pub bytes: Arc<[u8]>,
    /// who it's for (sorted), when not everyone on its topic
    pub audience: Option<Arc<Vec<i64>>>,
}

impl Event {
    pub fn is_for(&self, viewer: i64) -> bool {
        self.audience.as_ref().is_none_or(|a| a.binary_search(&viewer).is_ok())
    }
}

/// What a worker is told: events, or that a topic's audience changed (its connections whose
/// viewers aren't in it are closed).
pub enum Push {
    Event(Event),
    Revoke { topic: u64, audience: Arc<Vec<i64>> },
}

struct Topic {
    ring: VecDeque<Event>,
    /// the ring holds every event after this one (older ones were dropped)
    from: u64,
}

#[derive(Default)]
struct State {
    /// the last event's number
    last: u64,
    topics: HashMap<u64, Topic>,
}

pub struct Hub {
    /// the last event the published snapshot includes (pages built fresh carry it too)
    published: std::sync::atomic::AtomicU64,
    state: Mutex<State>,
    workers: Mutex<Vec<Inbox>>,
}

struct Inbox {
    queue: Arc<Mutex<Vec<Push>>>,
    waker: Arc<mio::Waker>,
}

/// The process's hub.
pub static HUB: std::sync::LazyLock<Hub> = std::sync::LazyLock::new(|| Hub { published: Default::default(), state: Mutex::default(), workers: Mutex::default() });

/// What a new stream gets first.
pub enum Replay {
    /// the events it missed, in order (possibly none), and whether it missed signals the ring
    /// no longer has (its page reloads what signals keep up to date: the stream gets `missed`)
    Events(Vec<Event>, bool),
    /// it missed more than the rings keep: reload
    Refresh,
}

impl Hub {
    /// A worker's inbox (once per worker, at start).
    pub fn register(&self, waker: Arc<mio::Waker>) -> Arc<Mutex<Vec<Push>>> {
        let queue = Arc::new(Mutex::new(Vec::new()));
        self.workers.lock().unwrap().push(Inbox { queue: queue.clone(), waker });
        queue
    }

    /// The last event's number (a new snapshot includes everything up to it).
    pub fn last(&self) -> u64 {
        self.state.lock().unwrap().last
    }

    /// The last event the published snapshot includes.
    pub fn published(&self) -> u64 {
        self.published.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A snapshot that includes up to event `seq` is published.
    pub fn set_published(&self, seq: u64) {
        self.published.store(seq, std::sync::atomic::Ordering::SeqCst);
    }

    /// The next `n` events' numbers: the first.
    pub fn reserve(&self, n: usize) -> u64 {
        let mut s = self.state.lock().unwrap();
        let first = s.last + 1;
        s.last += n as u64;
        first
    }

    /// Publishes events (numbered by `reserve`, in order) and audience changes.
    pub fn publish(&self, events: Vec<Event>, revokes: Vec<(u64, Arc<Vec<i64>>)>) {
        if events.is_empty() && revokes.is_empty() {
            return;
        }
        {
            let mut s = self.state.lock().unwrap();
            for e in &events {
                let t = s.topics.entry(e.topic).or_insert_with(|| Topic { ring: VecDeque::new(), from: 0 });
                t.ring.push_back(e.clone());
                while t.ring.len() > RING {
                    let old = t.ring.pop_front().unwrap();
                    t.from = old.id;
                }
            }
        }
        for w in self.workers.lock().unwrap().iter() {
            let mut q = w.queue.lock().unwrap();
            q.extend(events.iter().cloned().map(Push::Event));
            q.extend(revokes.iter().map(|(topic, audience)| Push::Revoke { topic: *topic, audience: audience.clone() }));
            drop(q);
            let _ = w.waker.wake();
        }
    }

    /// What `viewer`'s stream on `topics` missed after event `since`.
    pub fn replay(&self, topics: &[u64], since: u64, viewer: i64) -> Replay {
        let s = self.state.lock().unwrap();
        let (mut out, mut missed) = (vec![], false);
        for t in topics {
            if let Some(topic) = s.topics.get(t) {
                if since < topic.from {
                    if *t != SIGNALS {
                        return Replay::Refresh;
                    }
                    missed = true;
                }
                out.extend(topic.ring.iter().filter(|e| e.id > since && e.is_for(viewer)).cloned());
            }
        }
        out.sort_by_key(|e| e.id);
        Replay::Events(out, missed)
    }
}

/// An event's bytes: the Turbo Stream as server-sent event data (each line its own `data:`).
pub fn event_bytes(id: u64, stream: &[u8]) -> Arc<[u8]> {
    let mut b = Vec::with_capacity(stream.len() + 64);
    b.extend_from_slice(b"id: ");
    b.extend_from_slice(id.to_string().as_bytes());
    b.extend_from_slice(b"\nevent: message\n");
    for line in stream.split(|&c| c == b'\n') {
        b.extend_from_slice(b"data: ");
        b.extend_from_slice(line.strip_suffix(b"\r").unwrap_or(line));
        b.push(b'\n');
    }
    b.push(b'\n');
    b.into()
}

/// An event without a number (the browser's Last-Event-ID stays the one before).
pub fn unnumbered(stream: &[u8]) -> Vec<u8> {
    let mut b = event_bytes(0, stream).to_vec();
    b.drain(..b"id: 0\n".len());
    b
}

/// A Turbo Stream: `action` on `target` (an element's id), with `content` (append, prepend,
/// replace) or none (remove, refresh).
pub fn turbo_stream(action: &str, target: &str, content: Option<&[u8]>) -> Vec<u8> {
    let mut s = Vec::with_capacity(content.map(|c| c.len()).unwrap_or(0) + 96);
    s.extend_from_slice(b"<turbo-stream action=\"");
    s.extend_from_slice(action.as_bytes());
    s.extend_from_slice(b"\"");
    if !target.is_empty() {
        s.extend_from_slice(b" target=\"");
        crate::html::esc(&mut s, target);
        s.extend_from_slice(b"\"");
    }
    s.push(b'>');
    if let Some(c) = content {
        s.extend_from_slice(b"<template>");
        s.extend_from_slice(c);
        s.extend_from_slice(b"</template>");
    }
    s.extend_from_slice(b"</turbo-stream>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub() -> Hub {
        Hub { published: Default::default(), state: Mutex::default(), workers: Mutex::default() }
    }

    fn ev(h: &Hub, topic: u64) -> Event {
        let id = h.reserve(1);
        Event { id, topic, bytes: event_bytes(id, b"x"), audience: None }
    }

    fn ids(r: Replay) -> (Vec<u64>, bool) {
        match r {
            Replay::Events(es, missed) => (es.iter().map(|e| e.id).collect(), missed),
            Replay::Refresh => panic!("in the ring"),
        }
    }

    #[test]
    fn signals_replay_only_to_their_audience_and_past_the_ring_are_missed_not_refreshed() {
        let h = hub();
        let a = list_topic(0, 7);
        let mut s1 = ev(&h, SIGNALS);
        s1.audience = Some(Arc::new(vec![3, 5]));
        let s2 = ev(&h, SIGNALS);
        let e3 = ev(&h, a);
        h.publish(vec![s1, s2, e3], vec![]);
        assert_eq!(ids(h.replay(&[a, SIGNALS], 0, 5)), (vec![1, 2, 3], false));
        assert_eq!(ids(h.replay(&[a, SIGNALS], 0, 4)), (vec![2, 3], false), "not in its audience");
        let more: Vec<Event> = (0..RING + 5).map(|_| ev(&h, SIGNALS)).collect();
        h.publish(more, vec![]);
        let (got, missed) = ids(h.replay(&[a, SIGNALS], 0, 4));
        assert!(missed && got.len() == RING + 1, "the list's event, the signals the ring has, and missed");
        assert_eq!(unnumbered(b"<x>"), b"event: message\ndata: <x>\n\n");
    }

    #[test]
    fn a_stream_gets_what_it_missed_in_order_and_refresh_past_the_ring() {
        let h = hub();
        let (a, b) = (list_topic(0, 7), list_topic(0, 8));
        let e1 = ev(&h, a);
        let e2 = ev(&h, b);
        let e3 = ev(&h, a);
        h.publish(vec![e1, e2, e3], vec![]);
        assert_eq!(ids(h.replay(&[a], 1, 1)), (vec![3], false));
        assert_eq!(ids(h.replay(&[a, b], 0, 1)), (vec![1, 2, 3], false));
        let more: Vec<Event> = (0..RING + 5).map(|_| ev(&h, a)).collect();
        h.publish(more, vec![]);
        assert!(matches!(h.replay(&[a], 0, 1), Replay::Refresh), "past the ring");
        assert_eq!(ids(h.replay(&[a], h.last(), 1)), (vec![], false));
    }

    #[test]
    fn an_event_is_one_data_line_per_line() {
        let b = event_bytes(9, b"<a>\n<b>");
        assert_eq!(&*b, b"id: 9\nevent: message\ndata: <a>\ndata: <b>\n\n");
        assert_eq!(turbo_stream("remove", "m_1", None), b"<turbo-stream action=\"remove\" target=\"m_1\"></turbo-stream>");
    }
}
