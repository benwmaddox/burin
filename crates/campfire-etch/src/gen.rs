//! The fragments etchc generated (LLVM IR, linked under cross-language LTO), and the glue that
//! fills their input records from rows. The glue is temporary: step 5 generates the queries that
//! fill the records too.

#![allow(clippy::all, non_camel_case_types, dead_code)]
include!(env!("ETCH_GEN_RS"));

use crate::db::{Id, User};
use crate::render::MsgData;
use etch_rt::abi::Ctx;

pub fn viewer(u: &User) -> Viewer {
    Viewer { viewer: u.id, viewer_role: Str::of(&u.role) }
}

fn tmps(n: usize) -> (Vec<Vec<u8>>, Vec<*mut Vec<u8>>) {
    let mut bufs: Vec<Vec<u8>> = (0..n.max(1)).map(|_| Vec::with_capacity(256)).collect();
    let ptrs = bufs.iter_mut().map(|b| b as *mut Vec<u8>).collect();
    (bufs, ptrs)
}

/// The message's record: it points into `d`, so it lives no longer than `d`.
fn message_input(d: &MsgData) -> (In_message, Vec<In_message_0>) {
    let boosts: Vec<In_message_0> = d
        .boosts
        .iter()
        .map(|(b, name)| In_message_0 { b_booster: b.booster, b_booster_name: Str::of(name), b_content: Str::of(&b.content), b: b.id })
        .collect();
    let m = &d.m;
    let rec = In_message {
        m_creator: m.creator,
        m_client_id: Str::of(&m.client_id),
        m: m.id,
        m_created: m.created,
        m_creator_name: Str::of(&d.creator_name),
        m_room: m.room,
        m_body: Str::of(&m.body),
        list0: List::of(&[] as &[In_message_0]),
    };
    (rec, boosts)
}

/// The message's variant key for this viewer (etch_bits_message).
pub fn message_key(d: &MsgData, v: &Viewer) -> i64 {
    let (mut rec, boosts) = message_input(d);
    rec.list0 = List::of(&boosts);
    unsafe { etch_bits_message(&rec, v) }
}

pub fn message(ctx: &Ctx, d: &MsgData, v: &Viewer) -> Vec<u8> {
    let (mut rec, boosts) = message_input(d);
    rec.list0 = List::of(&boosts);
    let mut out = Vec::with_capacity(9000);
    let (_bufs, ptrs) = tmps(TMPS_MESSAGE);
    unsafe { etch_build_message(ctx, &rec, v, &mut out, ptrs.as_ptr()) };
    out
}

pub fn room_head(ctx: &Ctx, room: Id, name: Option<&str>) -> Vec<u8> {
    let rec = In_room_head { room_name: Str::opt(name), room };
    let v = Viewer { viewer: 0, viewer_role: Str::NONE };
    let mut out = Vec::with_capacity(4096);
    let (_bufs, ptrs) = tmps(TMPS_ROOM_HEAD);
    unsafe { etch_build_room_head(ctx, &rec, &v, &mut out, ptrs.as_ptr()) };
    out
}

pub fn room_tail(ctx: &Ctx, room: Id) -> Vec<u8> {
    let rec = In_room_tail { room };
    let v = Viewer { viewer: 0, viewer_role: Str::NONE };
    let mut out = Vec::with_capacity(4096);
    let (_bufs, ptrs) = tmps(TMPS_ROOM_TAIL);
    unsafe { etch_build_room_tail(ctx, &rec, &v, &mut out, ptrs.as_ptr()) };
    out
}
