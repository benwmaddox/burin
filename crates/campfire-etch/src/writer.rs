//! The writer: the one thread that changes data (DESIGN.md principle 3). It takes actions from a
//! bounded queue, runs every queued action in one transaction (group commit), then builds what the
//! commit changed and publishes the next snapshot. Readers never wait on it.
//!
//! What each action changes is known here, not found by parsing SQL: post_message adds a message
//! to its room (one new fragment, the oldest dropped), touches the room, and marks the room
//! unread for some members (their sidebars are rebuilt).

use crate::db::{self, Id};
use crate::render::Ctx;
use crate::state::{self, Snap};
use arc_swap::ArcSwap;
use rusqlite::{params, Connection};
use std::collections::HashSet;
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
}

/// Sends a command, refusing when the queue is full (backpressure: the caller answers 503).
pub fn send(tx: &SyncSender<Cmd>, cmd: Cmd) -> bool {
    match tx.try_send(cmd) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
    }
}

pub fn run(mut c: Connection, ctx: Arc<Ctx>, snap: Arc<ArcSwap<Snap>>, rx: Receiver<Cmd>, stats: Arc<Stats>) {
    let mut next_id: Id = c.query_row("SELECT coalesce(max(id), 0) + 1 FROM Message", [], |r| r.get(0)).expect("max id");
    let mut seq: i64 = c
        .query_row(
            "SELECT max(s) FROM (SELECT max(seq) s FROM Message UNION ALL SELECT max(seq) FROM Room UNION ALL SELECT max(seq) FROM Membership UNION ALL SELECT max(seq) FROM Boost UNION ALL SELECT max(seq) FROM \"User\" UNION ALL SELECT max(seq) FROM Session)",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .expect("max seq")
        .unwrap_or(0);
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
        let mut posted: Vec<db::Message> = vec![];
        let mut unread_users: HashSet<Id> = HashSet::new();
        let mut warm_rooms: HashSet<Id> = HashSet::new();
        let mut warm_sidebars: HashSet<Id> = HashSet::new();
        // ---- the transaction: every queued action
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).expect("begin");
        for cmd in batch.drain(..) {
            match cmd {
                Cmd::Post { actor, room, body, reply } => {
                    let r = post_message(&tx, actor, room, &body, &mut next_id, &mut seq, &mut unread_users);
                    match r {
                        Ok(m) => {
                            replies.push((reply, Ok(m.id)));
                            posted.push(m);
                        }
                        Err(e) => replies.push((reply, Err(e))),
                    }
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
        // ---- what the commit changed, built into the next snapshot
        let mut next = cur.next();
        let mut touched_rooms: HashSet<Id> = HashSet::new();
        for m in posted {
            let rid = m.room;
            touched_rooms.insert(rid);
            if let Some(view) = next.rooms.get(&rid).cloned() {
                let frag = match state::build_msgs(&c, &ctx, &[m.id], &view.members).pop() {
                    Some(f) => Arc::new(f),
                    None => continue,
                };
                let mut msgs = view.msgs.clone();
                msgs.push(frag);
                if msgs.len() > 40 {
                    msgs.remove(0);
                }
                next.rooms.insert(rid, Arc::new(state::RoomView {
                    id: view.id,
                    members: view.members.clone(),
                    head: view.head.clone(),
                    tail: view.tail.clone(),
                    tok_seg: view.tok_seg.clone(),
                    tok: crate::render::RoomTokens { plain: view.tok.plain.clone(), composer: view.tok.composer.clone(), picker: view.tok.picker.clone() },
                    headers: view.headers.clone(),
                    msgs,
                }));
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
        // sidebars: the members marked unread, and every member of a touched direct room (their
        // order changed); only those already kept
        let mut rebuild: HashSet<Id> = unread_users.into_iter().filter(|u| next.sidebars.contains_key(u)).collect();
        for rid in &touched_rooms {
            if let Ok(Some(room)) = db::room(&c, *rid) {
                if room.direct {
                    for u in db::members(&c, *rid).unwrap_or_default() {
                        if next.sidebars.contains_key(&u) {
                            rebuild.insert(u);
                        }
                    }
                }
            }
        }
        for u in warm_sidebars {
            if !next.sidebars.contains_key(&u) {
                rebuild.insert(u);
            }
        }
        for u in rebuild {
            if let Ok(s) = state::build_sidebar(&c, &ctx, &next.users, u) {
                next.sidebars.insert(u, Arc::new(s));
                stats.rebuilt_sidebars.fetch_add(1, Ordering::Relaxed);
            }
        }
        snap.store(Arc::new(next));
        stats.batches.fetch_add(1, Ordering::Relaxed);
        stats.posts.fetch_add(replies.iter().filter(|r| r.1.is_ok()).count() as u64, Ordering::Relaxed);
        for (to, r) in replies {
            let _ = to.send(r);
        }
        // the connection may be borrowed mutably again next round
        let _ = &mut c;
    }
}

/// post_message(room, body): `require member(actor, room)`, `require not blank(body)`, create
/// the message, touch its room, and (the trigger) mark the room unread for the members who
/// aren't connected.
fn post_message(c: &Connection, actor: Id, room: Id, body: &str, next_id: &mut Id, seq: &mut i64, unread: &mut HashSet<Id>) -> Result<db::Message, &'static str> {
    let member: bool = c
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM Membership WHERE room = ? AND \"user\" = ?)")
        .and_then(|mut s| s.query_row(params![room, actor], |r| r.get(0)))
        .map_err(|_| "error")?;
    if !member {
        return Err("not_found");
    }
    if body.trim().is_empty() {
        return Err("invalid");
    }
    let now = etch_rt::time::now_ms();
    let id = *next_id;
    *next_id += 1;
    *seq += 1;
    let client_id = token(36);
    c.prepare_cached("INSERT INTO Message (id, seq, created, updated, room, creator, body, client_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .and_then(|mut s| s.execute(params![id, *seq, now, now, room, actor, body, client_id]))
        .map_err(|_| "error")?;
    c.prepare_cached("UPDATE Room SET updated = ? WHERE id = ?").and_then(|mut s| s.execute(params![now, room])).map_err(|_| "error")?;
    // the trigger: members other than the poster, visible, not connected in the last minute
    {
        let mut s = c
            .prepare_cached(
                "SELECT \"user\" FROM Membership WHERE room = ? AND \"user\" IS NOT ? AND involvement IS NOT 'invisible' \
                 AND (connected_at IS NULL OR connected_at < ?)",
            )
            .map_err(|_| "error")?;
        let users = s.query_map(params![room, actor, now - 60_000], |r| r.get::<_, Id>(0)).map_err(|_| "error")?;
        for u in users.flatten() {
            unread.insert(u);
        }
    }
    c.prepare_cached(
        "UPDATE Membership SET unread_since = ?, unread_since__seq = ? WHERE room = ? AND \"user\" IS NOT ? AND involvement IS NOT 'invisible' \
         AND (connected_at IS NULL OR connected_at < ?)",
    )
    .and_then(|mut s| s.execute(params![now, *seq, room, actor, now - 60_000]))
    .map_err(|_| "error")?;
    Ok(db::Message { id, room, creator: actor, created: now, seq: *seq, body: body.to_string(), client_id, has_attachment: false })
}

/// A random token of n URL-safe characters.
fn token(n: usize) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut raw = vec![0u8; n];
    getrandom::getrandom(&mut raw).expect("random");
    raw.iter().map(|b| A[(*b & 63) as usize] as char).collect()
}
