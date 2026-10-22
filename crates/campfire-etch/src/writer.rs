//! The writer: the one thread that changes data (DESIGN.md principle 3). It takes actions from a
//! bounded queue, runs every queued action in one transaction (group commit), then builds what the
//! commit changed and publishes the next snapshot. Readers never wait on it.
//!
//! What each action changes is known here, not found by parsing SQL: post_message adds a message
//! to its room (one new fragment, the oldest dropped), touches the room, and marks the room
//! unread for some members (their sidebars are rebuilt).

use crate::db::{self, Id};
use crate::gen;
use etch_rt::abi::{Deltas, Writer};
use crate::render::Ctx;
use crate::state::{self, Snap};
use arc_swap::ArcSwap;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::Arc;

pub enum Cmd {
    Post { actor: Id, room: Id, body: String, reply: SyncSender<Result<Id, &'static str>> },
    /// precompute this room (a cold read found it missing)
    WarmRoom(Id),
    /// precompute this user's sidebar
    WarmSidebar(Id),
}

pub const QUEUE: usize = 1024;

#[derive(Default)]
pub struct Stats {
    pub batches: AtomicU64,
    pub posts: AtomicU64,
    pub rebuilt_sidebars: AtomicU64,
    /// registers loaded from SQLite after a commit (a read after the write)
    pub reads: AtomicU64,
    /// registers built from records the action forwarded, or from patched kept records: no read
    pub forwarded: AtomicU64,
    pub patched: AtomicU64,
}

/// Sends a command, refusing when the queue is full (backpressure: the caller answers 503).
pub fn send(tx: &SyncSender<Cmd>, cmd: Cmd) -> bool {
    match tx.try_send(cmd) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
    }
}

pub fn run(mut c: Connection, ctx: Arc<Ctx>, snap: Arc<ArcSwap<Snap>>, rx: Receiver<Cmd>, stats: Arc<Stats>) {
    let seq: i64 = c
        .query_row(
            "SELECT max(s) FROM (SELECT max(seq) s FROM Message UNION ALL SELECT max(seq) FROM Room UNION ALL SELECT max(seq) FROM Membership UNION ALL SELECT max(seq) FROM Boost UNION ALL SELECT max(seq) FROM \"User\" UNION ALL SELECT max(seq) FROM Session)",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .expect("max seq")
        .unwrap_or(0);
    let mut w = Writer::new(seq);
    // what the writer keeps between batches: viewers' records, and sidebars' records (patched in place)
    let mut viewers: HashMap<Id, gen::Kept<gen::Viewer>> = HashMap::new();
    let mut sides: HashMap<Id, gen::Kept<gen::In_sidebar>> = HashMap::new();
    let mut batch = Vec::with_capacity(256);
    while let Ok(first) = rx.recv() {
        batch.push(first);
        while batch.len() < 256 {
            match rx.try_recv() {
                Ok(c) => batch.push(c),
                Err(_) => break,
            }
        }
        let cur = snap.load_full();
        let mut replies: Vec<(SyncSender<Result<Id, &'static str>>, Result<Id, &'static str>)> = vec![];
        let mut deltas = Deltas::default();
        let mut warm_rooms: HashSet<Id> = HashSet::new();
        let mut warm_sidebars: HashSet<Id> = HashSet::new();
        // ---- the transaction: every queued action, each the generated rule
        w.begin(etch_rt::time::now_ms());
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).expect("begin");
        for cmd in batch.drain(..) {
            match cmd {
                Cmd::Post { actor, room, body, reply } => {
                    let vrec = viewers.entry(actor).or_insert_with(|| gen::kept_viewer(&tx, &ctx.abi, actor));
                    let r = gen::post_message(&tx, &ctx.abi, &mut w, actor, &vrec.rec, room, &body, &mut deltas);
                    replies.push((reply, r.map(|_| 0)));
                }
                Cmd::WarmRoom(r) => {
                    warm_rooms.insert(r);
                }
                Cmd::WarmSidebar(u) => {
                    warm_sidebars.insert(u);
                }
            }
        }
        tx.commit().expect("commit");
        // ---- the delta edges the actions reported, built into the next snapshot
        let mut next = cur.next();
        let mut reload: HashSet<Id> = HashSet::new();
        let mut dirty: HashSet<Id> = HashSet::new();
        let mut forwarded: HashMap<Id, i64> = HashMap::new();
        let mut touched_rooms: HashSet<Id> = HashSet::new();
        for &(op, a, key, item) in &deltas.list {
            let reg = |a: i64| gen::REGISTERS.get(a as usize).copied().unwrap_or("");
            match op {
                1 => {}
                // a new row's record, filled by the action: no read
                6 => {
                    forwarded.insert(key, item);
                }
                2 => {
                    touched_rooms.insert(key);
                    if let Some(view) = next.rooms.get(&key).cloned() {
                        let frag = match forwarded.get(&item) {
                            Some(rec) => {
                                for u in &view.members {
                                    viewers.entry(*u).or_insert_with(|| gen::kept_viewer(&c, &ctx.abi, *u));
                                }
                                let vs: Vec<&gen::Viewer> = view.members.iter().map(|u| &*viewers[u].rec).collect();
                                let (variants, by_member) = gen::message_from(&ctx.abi, *rec as *const gen::In_message, &vs);
                                stats.forwarded.fetch_add(1, Ordering::Relaxed);
                                Arc::new(state::MsgFrag { id: item, variants: variants.into_iter().map(state::Seg::new).collect(), by_member })
                            }
                            None => {
                                stats.reads.fetch_add(1, Ordering::Relaxed);
                                match state::build_msgs(&c, &ctx, &[item], &view.members).pop() {
                                    Some(f) => Arc::new(f),
                                    None => continue,
                                }
                            }
                        };
                        let mut msgs = view.msgs.clone();
                        msgs.push(frag);
                        if msgs.len() > 40 {
                            msgs.remove(0);
                        }
                        next.rooms.insert(key, Arc::new(state::RoomView {
                            id: view.id,
                            members: view.members.clone(),
                            head: view.head.clone(),
                            tail: view.tail.clone(),
                            tok_seg: view.tok_seg.clone(),
                            tok: view.tok.clone(),
                            headers: view.headers.clone(),
                            msgs,
                        }));
                    }
                }
                3 if reg(a) == "sidebar" => {
                    if sides.contains_key(&key) {
                        reload.insert(key);
                    }
                }
                4 if reg(a) == "sidebar" => {
                    reload.extend(sides.keys().copied());
                }
                _ => next.rooms.clear(),
            }
        }
        // patches of kept records: the item's field set in place, then the bytes rebuilt
        // (an item is in one of the lists a patch may name: reloaded only if none holds it)
        let mut found: HashMap<(Id, i64), bool> = HashMap::new();
        for &(reg, key, item, list, field, value) in &deltas.patches {
            if let Some(k) = sides.get_mut(&key) {
                let hit = unsafe { gen::patch(reg, list, field, &mut *k.rec as *mut gen::In_sidebar as *mut u8, item, value) };
                *found.entry((key, item)).or_insert(false) |= hit;
            }
        }
        for ((key, _), hit) in found {
            if hit {
                dirty.insert(key);
            } else {
                reload.insert(key);
            }
        }
        // a written row that orders kept lists: only the kept records holding it are reloaded
        for &(reg, row, list, field) in &deltas.finds {
            for (uid, k) in sides.iter_mut() {
                if unsafe { gen::find(reg, list, field, &mut *k.rec as *mut gen::In_sidebar as *mut u8, row) } {
                    reload.insert(*uid);
                }
            }
        }
        // a room whose page might now differ for another reason (the invitation) is rebuilt
        for rid in &touched_rooms {
            if next.rooms.contains_key(rid) {
                if let Ok(Some(room)) = db::room(&c, *rid) {
                    if !state::precomputable(&c, &room).unwrap_or(false) {
                        next.rooms.remove(rid);
                    }
                }
            }
        }
        for rid in warm_rooms {
            if !next.rooms.contains_key(&rid) {
                if let Ok(Some(v)) = state::build_room(&c, &ctx, &next.host.clone(), &next.users, rid) {
                    next.rooms.insert(rid, Arc::new(v));
                }
            }
        }
        for u in warm_sidebars {
            if !sides.contains_key(&u) {
                reload.insert(u);
            }
        }
        for u in &reload {
            if let Some(k) = gen::kept_sidebar(&c, &ctx.abi, *u) {
                sides.insert(*u, k);
                dirty.insert(*u);
                stats.reads.fetch_add(1, Ordering::Relaxed);
            }
        }
        for u in dirty {
            if let Some(k) = sides.get(&u) {
                let v = viewers.entry(u).or_insert_with(|| gen::kept_viewer(&c, &ctx.abi, u));
                let pieces = gen::sidebar_bytes(&ctx.abi, k, &v.rec);
                next.sidebars.insert(u, Arc::new(state::Sidebar { pieces: pieces.into_iter().map(state::Seg::new).collect() }));
                stats.rebuilt_sidebars.fetch_add(1, Ordering::Relaxed);
                if !reload.contains(&u) {
                    stats.patched.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        snap.store(Arc::new(next));
        stats.batches.fetch_add(1, Ordering::Relaxed);
        stats.posts.fetch_add(replies.iter().filter(|r| r.1.is_ok()).count() as u64, Ordering::Relaxed);
        for (to, r) in replies {
            let _ = to.send(r);
        }
        let _ = &mut c;
    }
}
