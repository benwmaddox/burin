//! The registers etchc generated (LLVM IR, linked under cross-language LTO): their loaders (SQL
//! straight to records), variant keys and builders. What's left here is calling them: each thread
//! keeps its prepared statements (per connection) and an arena for the records it reads.

#![allow(clippy::all, non_camel_case_types, dead_code)]
include!(env!("ETCH_GEN_RS"));

use crate::db::{Id, User};
use etch_rt::abi::{Arena, Ctx, Db};
use rusqlite::Connection;
use std::cell::RefCell;

thread_local! {
    /// this thread's prepared statements, for the connection they were prepared on
    static DB: RefCell<Option<(usize, Db)>> = const { RefCell::new(None) };
    static ARENA: RefCell<Arena> = RefCell::new(Arena::default());
}

/// Runs `f` with this thread's statements for connection `c` and an arena emptied for it.
fn with<T>(c: &Connection, f: impl FnOnce(*mut Db, *mut Arena) -> T) -> T {
    let h = unsafe { c.handle() };
    DB.with(|d| {
        let mut d = d.borrow_mut();
        if d.as_ref().map(|(k, _)| *k != h as usize).unwrap_or(true) {
            *d = Some((h as usize, unsafe { Db::new(h) }));
        }
        let db: *mut Db = &mut d.as_mut().unwrap().1;
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            a.reset();
            f(db, &mut *a)
        })
    })
}

pub fn viewer(u: &User) -> Viewer {
    Viewer { viewer: u.id, viewer_role: Str::of(&u.role) }
}

fn tmps(n: usize) -> (Vec<Vec<u8>>, Vec<*mut Vec<u8>>) {
    let mut bufs: Vec<Vec<u8>> = (0..n.max(1)).map(|_| Vec::with_capacity(256)).collect();
    let ptrs = bufs.iter_mut().map(|b| b as *mut Vec<u8>).collect();
    (bufs, ptrs)
}

/// A message's variants for these viewers: its record loaded once (etch_load_message), then a
/// variant per distinct key (etch_bits_message), built by etch_build_message. None if it's gone.
pub fn message_variants(c: &Connection, ctx: &Ctx, id: Id, viewers: &[Viewer]) -> Option<(Vec<Vec<u8>>, Vec<u8>)> {
    with(c, |db, arena| {
        let mut rec = std::mem::MaybeUninit::<In_message>::zeroed();
        if !unsafe { etch_load_message(ctx, db, id, rec.as_mut_ptr(), arena) } {
            return None;
        }
        let rec = unsafe { rec.assume_init() };
        let (_bufs, ptrs) = tmps(TMPS_MESSAGE);
        let mut keys: Vec<i64> = vec![];
        let mut variants = vec![];
        let mut by = Vec::with_capacity(viewers.len());
        for v in viewers {
            let k = unsafe { etch_bits_message(&rec, v) };
            let i = match keys.iter().position(|x| *x == k) {
                Some(i) => i,
                None => {
                    keys.push(k);
                    let mut out = Vec::with_capacity(9000);
                    unsafe { etch_build_message(ctx, &rec, v, &mut out, ptrs.as_ptr()) };
                    variants.push(out);
                    keys.len() - 1
                }
            };
            by.push(i as u8);
        }
        Some((variants, by))
    })
}

pub fn room_head(c: &Connection, ctx: &Ctx, room: Id) -> Option<Vec<u8>> {
    with(c, |db, arena| {
        let mut rec = std::mem::MaybeUninit::<In_room_head>::zeroed();
        if !unsafe { etch_load_room_head(ctx, db, room, rec.as_mut_ptr(), arena) } {
            return None;
        }
        let rec = unsafe { rec.assume_init() };
        let v = Viewer { viewer: 0, viewer_role: Str::NONE };
        let mut out = Vec::with_capacity(4096);
        let (_bufs, ptrs) = tmps(TMPS_ROOM_HEAD);
        unsafe { etch_build_room_head(ctx, &rec, &v, &mut out, ptrs.as_ptr()) };
        Some(out)
    })
}

pub fn room_tail(c: &Connection, ctx: &Ctx, room: Id) -> Option<Vec<u8>> {
    with(c, |db, arena| {
        let mut rec = std::mem::MaybeUninit::<In_room_tail>::zeroed();
        if !unsafe { etch_load_room_tail(ctx, db, room, rec.as_mut_ptr(), arena) } {
            return None;
        }
        let rec = unsafe { rec.assume_init() };
        let v = Viewer { viewer: 0, viewer_role: Str::NONE };
        let mut out = Vec::with_capacity(4096);
        let (_bufs, ptrs) = tmps(TMPS_ROOM_TAIL);
        unsafe { etch_build_room_tail(ctx, &rec, &v, &mut out, ptrs.as_ptr()) };
        Some(out)
    })
}
