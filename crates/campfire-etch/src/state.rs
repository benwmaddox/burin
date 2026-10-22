//! The snapshot: everything the fast path reads, precomputed, immutable once published. The
//! writer builds the next one and swaps a pointer (arc-swap); readers never lock.
//!
//! All of it is derived from SQLite and can be rebuilt from it at any time.

use crate::db::{self, Id, User};
use crate::gen;
use crate::render::{self, Ctx, RoomTokens, SideData, SideDirect};
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Arc;

pub struct Snap {
    pub version: u64,
    /// the host the fragments were built for (the "Copy link" buttons name it)
    pub host: String,
    pub sessions: HashMap<Box<[u8]>, Id>,
    pub users: HashMap<Id, User>,
    pub rooms: HashMap<Id, Arc<RoomView>>,
    pub sidebars: HashMap<Id, Arc<Sidebar>>,
    pub end: Arc<Seg>,
}

impl Snap {
    /// A copy sharing every room and sidebar (the writer changes only what a commit touched).
    pub fn next(&self) -> Snap {
        Snap {
            version: self.version + 1,
            host: self.host.clone(),
            sessions: self.sessions.clone(),
            users: self.users.clone(),
            rooms: self.rooms.clone(),
            sidebars: self.sidebars.clone(),
            end: self.end.clone(),
        }
    }
}

/// A piece of a page, as it is and precompressed (built once, at write time).
pub struct Seg {
    pub raw: Vec<u8>,
    pub gz: etch_rt::gz::Piece,
}

impl Seg {
    pub fn new(raw: Vec<u8>) -> Seg {
        let gz = etch_rt::gz::Piece::new(&raw);
        Seg { raw, gz }
    }
}

pub struct RoomView {
    pub id: Id,
    /// the members' user ids, sorted: a viewer's place picks each message's variant
    pub members: Vec<Id>,
    pub head: Arc<Seg>,
    pub tail: Arc<Seg>,
    pub tok: RoomTokens,
    /// the page's plain token, spliced into the sidebar
    pub tok_seg: Arc<Seg>,
    /// the response's headers after the length: type, the remembered room, Vary
    pub headers: Vec<u8>,
    pub msgs: Vec<Arc<MsgFrag>>,
}

pub struct MsgFrag {
    pub id: Id,
    /// each distinct variant the room's members see
    pub variants: Vec<Seg>,
    /// for each member (in `RoomView::members` order), the variant they see
    pub by_member: Vec<u8>,
}

pub struct Sidebar {
    /// the sidebar between the page's plain tokens
    pub pieces: Vec<Seg>,
}

/// What the room page needs that the snapshot doesn't keep per room: phase 1a precomputes only
/// rooms whose page is the same for every member (not direct rooms, and not the first room while
/// it shows the invitation).
pub fn precomputable(c: &Connection, room: &db::Room) -> rusqlite::Result<bool> {
    if room.direct {
        return Ok(false);
    }
    if db::original_room(c)? == Some(room.id) && db::message_count(c, room.id)? <= 40 {
        return Ok(false);
    }
    Ok(true)
}

/// A message's fragment for a room's members: loaded, keyed and built by the generated code.
pub fn build_msg(c: &Connection, ctx: &Ctx, id: Id, members: &[Id], users: &HashMap<Id, User>) -> Option<MsgFrag> {
    let viewers: Vec<gen::Viewer> = members
        .iter()
        .map(|uid| match users.get(uid) {
            Some(u) => gen::viewer(u),
            None => gen::Viewer { viewer: *uid, viewer_role: etch_rt::abi::Str::NONE },
        })
        .collect();
    let (variants, by_member) = gen::message_variants(c, &ctx.abi, id, &viewers)?;
    Some(MsgFrag { id, variants: variants.into_iter().map(Seg::new).collect(), by_member })
}

/// The room's view, from SQLite; None when phase 1a doesn't precompute it.
pub fn build_room(c: &Connection, ctx: &Ctx, host: &str, users: &HashMap<Id, User>, room_id: Id) -> rusqlite::Result<Option<RoomView>> {
    let room = match db::room(c, room_id)? {
        Some(r) => r,
        None => return Ok(None),
    };
    if !precomputable(c, &room)? {
        return Ok(None);
    }
    let account = db::account(c)?;
    let members = db::members(c, room_id)?;
    let tok = render::room_tokens(ctx, room_id);
    assert!(!account.has_logo, "phase 1: an account logo isn't supported");
    let _ = host;
    let head = gen::room_head(c, &ctx.abi, room_id).expect("the room");
    let tail = gen::room_tail(c, &ctx.abi, room_id).expect("the room");
    let msgs = db::last_messages(c, room_id)?;
    let frags = msgs.iter().filter_map(|m| build_msg(c, ctx, m.id, &members, users).map(Arc::new)).collect();
    let headers = format!("Content-Type: text/html; charset=utf-8\r\nSet-Cookie: last_room={room_id}; Path=/; Max-Age=31536000\r\nVary: Accept-Encoding\r\n").into_bytes();
    let tok_seg = Arc::new(Seg::new(tok.plain.as_bytes().to_vec()));
    Ok(Some(RoomView { id: room_id, members, head: Arc::new(Seg::new(head)), tail: Arc::new(Seg::new(tail)), tok, tok_seg, headers, msgs: frags }))
}

pub fn side_data(c: &Connection, users: &HashMap<Id, User>, uid: Id) -> rusqlite::Result<SideData> {
    let account = db::account(c)?;
    let me = users.get(&uid);
    let viewer_name = me.map(|u| u.name.clone()).unwrap_or_default();
    let all = db::sidebar_rooms(c, uid)?;
    let mut directs = vec![];
    let mut rooms = vec![];
    for r in all {
        if r.room.direct {
            directs.push(r);
        } else {
            rooms.push(r);
        }
    }
    // directs: most recently updated first (a stable sort keeps the name order for ties)
    directs.sort_by(|a, b| b.room.updated.cmp(&a.room.updated));
    let mut ds = vec![];
    for r in directs {
        let others: Vec<Id> = db::other_members(c, r.room.id, uid, i64::MAX)?;
        let by_membership: Vec<String> = others.iter().map(|id| users.get(id).map(|u| u.name.clone()).unwrap_or_default()).collect();
        let by_created = db::other_member_names_by_user_created(c, r.room.id, uid)?;
        let label = render::direct_label(&by_membership, &by_created, &viewer_name);
        let avatars = others.into_iter().take(4).collect();
        ds.push(SideDirect { room: r, avatars, label });
    }
    Ok(SideData {
        user: uid,
        directs: ds,
        rooms,
        placeholders: db::direct_placeholders(c, uid)?,
        may_create: !account.restrict_room_creation || me.map(|u| u.admin).unwrap_or(false),
    })
}

pub fn build_sidebar(c: &Connection, ctx: &Ctx, users: &HashMap<Id, User>, uid: Id) -> rusqlite::Result<Sidebar> {
    Ok(Sidebar { pieces: render::sidebar(ctx, &side_data(c, users, uid)?).into_iter().map(Seg::new).collect() })
}

/// The page's body, piece by piece, for the member at `place` in the room's members.
/// The fast path and the golden-model check both assemble pages with it.
pub fn assemble<'a>(room: &'a RoomView, place: usize, side: &'a Sidebar, end: &'a Seg, mut push: impl FnMut(&'a Seg)) {
    push(&room.head);
    for m in &room.msgs {
        push(&m.variants[m.by_member[place] as usize]);
    }
    push(&room.tail);
    for (i, p) in side.pieces.iter().enumerate() {
        if i > 0 {
            push(&room.tok_seg);
        }
        push(p);
    }
    push(end);
}

/// The users, as the snapshot keeps them.
pub fn load_users(c: &Connection) -> rusqlite::Result<HashMap<Id, User>> {
    Ok(db::users(c)?.into_iter().map(|u| (u.id, u)).collect())
}

pub fn load_sessions(c: &Connection) -> rusqlite::Result<HashMap<Box<[u8]>, Id>> {
    Ok(db::sessions(c)?.into_iter().map(|(t, u)| (t.into_bytes().into_boxed_slice(), u)).collect())
}
