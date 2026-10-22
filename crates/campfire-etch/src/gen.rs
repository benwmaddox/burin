//! The registers etchc generated (LLVM IR, linked under cross-language LTO): their loaders (SQL
//! straight to records), variant keys and builders. What's left here is calling them: each thread
//! keeps its prepared statements (per connection) and an arena for the records it reads.

#![allow(clippy::all, non_camel_case_types, dead_code)]
include!(env!("ETCH_GEN_RS"));

use crate::db::Id;
use etch_rt::abi::{Arena, Ctx, Db};
use rusqlite::Connection;
use std::cell::RefCell;
use std::mem::MaybeUninit;

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

fn tmps(n: usize) -> (Vec<Vec<u8>>, Vec<*mut Vec<u8>>) {
    let mut bufs: Vec<Vec<u8>> = (0..n.max(1)).map(|_| Vec::with_capacity(256)).collect();
    let ptrs = bufs.iter_mut().map(|b| b as *mut Vec<u8>).collect();
    (bufs, ptrs)
}

/// The viewers' records (etch_load_viewer), for building their variants.
unsafe fn viewers(ctx: &Ctx, db: *mut Db, arena: *mut Arena, ids: &[Id]) -> Vec<Viewer> {
    ids.iter()
        .map(|id| {
            let mut v = MaybeUninit::<Viewer>::zeroed();
            etch_load_viewer(ctx, db, *id, v.as_mut_ptr(), arena);
            v.assume_init()
        })
        .collect()
}

/// Messages' variants for these viewers: each record loaded once (etch_load_message), then a
/// variant per distinct key (etch_bits_message), built by etch_build_message. None for one gone.
pub fn messages(c: &Connection, ctx: &Ctx, ids: &[Id], audience: &[Id]) -> Vec<Option<(Vec<Vec<u8>>, Vec<u8>)>> {
    with(c, |db, arena| unsafe {
        let vs = viewers(ctx, db, arena, audience);
        let (_bufs, ptrs) = tmps(TMPS_MESSAGE);
        let mut splits = vec![];
        ids.iter()
            .map(|id| {
                let mut rec = MaybeUninit::<In_message>::zeroed();
                if !etch_load_message(ctx, db, *id, rec.as_mut_ptr(), arena) {
                    return None;
                }
                let rec = rec.assume_init();
                let mut keys: Vec<i64> = vec![];
                let mut variants = vec![];
                let mut by = Vec::with_capacity(vs.len());
                for v in &vs {
                    let k = etch_bits_message(&rec, v);
                    let i = match keys.iter().position(|x| *x == k) {
                        Some(i) => i,
                        None => {
                            keys.push(k);
                            let mut out = Vec::with_capacity(9000);
                            etch_build_message(ctx, &rec, v, &mut out, ptrs.as_ptr(), &mut splits);
                            variants.push(out);
                            keys.len() - 1
                        }
                    };
                    by.push(i as u8);
                }
                Some((variants, by))
            })
            .collect()
    })
}

/// A register keyed by a row with no variants (the room's head and tail, splices).
macro_rules! plain {
    ($f:ident, $rec:ident, $load:ident, $build:ident, $tmps:ident) => {
        pub fn $f(c: &Connection, ctx: &Ctx, key: Id) -> Option<Vec<u8>> {
            with(c, |db, arena| unsafe {
                let mut rec = MaybeUninit::<$rec>::zeroed();
                if !$load(ctx, db, key, rec.as_mut_ptr(), arena) {
                    return None;
                }
                let rec = rec.assume_init();
                let v = MaybeUninit::<Viewer>::zeroed().assume_init();
                let mut out = Vec::with_capacity(4096);
                let (_bufs, ptrs) = tmps($tmps);
                let mut splits = vec![];
                $build(ctx, &rec, &v, &mut out, ptrs.as_ptr(), &mut splits);
                Some(out)
            })
        }
    };
}

plain!(room_head, In_room_head, etch_load_room_head, etch_build_room_head, TMPS_ROOM_HEAD);
plain!(room_tail, In_room_tail, etch_load_room_tail, etch_build_room_tail, TMPS_ROOM_TAIL);
plain!(sidebar_splice0, In_sidebar__s0, etch_load_sidebar__s0, etch_build_sidebar__s0, TMPS_SIDEBAR__S0);

/// The viewer's sidebar (keyed by the viewer), in pieces between its splices.
pub fn sidebar(c: &Connection, ctx: &Ctx, viewer: Id) -> Option<Vec<Vec<u8>>> {
    with(c, |db, arena| unsafe {
        let v = viewers(ctx, db, arena, &[viewer]).remove(0);
        let mut rec = MaybeUninit::<In_sidebar>::zeroed();
        if !etch_load_sidebar(ctx, db, viewer, rec.as_mut_ptr(), arena) {
            return None;
        }
        let rec = rec.assume_init();
        let mut out = Vec::with_capacity(8192);
        let (_bufs, ptrs) = tmps(TMPS_SIDEBAR);
        let mut splits = vec![];
        etch_build_sidebar(ctx, &rec, &v, &mut out, ptrs.as_ptr(), &mut splits);
        let mut pieces = vec![];
        let mut at = 0;
        for s in splits {
            pieces.push(out[at..s].to_vec());
            at = s;
        }
        pieces.push(out[at..].to_vec());
        Some(pieces)
    })
}

/// post_message, as the generated rule (etch_act_post_message), inside the writer's transaction.
pub fn post_message(c: &Connection, ctx: &Ctx, w: &mut etch_rt::abi::Writer, viewer: Id, room: Id, body: &str, d: &mut etch_rt::abi::Deltas) -> Result<(), &'static str> {
    with(c, |db, _arena| unsafe {
        let args = Args_post_message { room, body: Str::of(body) };
        let r = etch_act_post_message(ctx, db, w, viewer, &args, d);
        if r == 0 {
            Ok(())
        } else {
            Err(OUTCOMES_POST_MESSAGE[r as usize])
        }
    })
}
