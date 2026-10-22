# One-off: kept records and forwarding in campfire-etch's glue (gen.rs).
p = 'D:/code/etch/crates/campfire-etch/src/gen.rs'
s = open(p, encoding='utf-8').read()
i = s.index('/// post_message, as the generated rule')
s = s[:i] + r'''/// A record the writer keeps between batches (its text in its own arena).
pub struct Kept<T> {
    pub rec: Box<T>,
    _arena: Box<Arena>,
}

/// Runs `f` with this thread's statements for `c` (no arena: the caller brings one).
fn with_db<T>(c: &Connection, f: impl FnOnce(*mut Db) -> T) -> T {
    with(c, |db, _| f(db))
}

/// A viewer's record, kept (a read, once per viewer).
pub fn kept_viewer(c: &Connection, ctx: &Ctx, id: Id) -> Kept<Viewer> {
    let mut arena = Box::new(Arena::new());
    let mut rec: Box<Viewer> = Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
    with_db(c, |db| unsafe { etch_load_viewer(ctx, db, id, &mut *rec, &mut *arena) });
    Kept { rec, _arena: arena }
}

/// A viewer's sidebar record, kept (a read).
pub fn kept_sidebar(c: &Connection, ctx: &Ctx, uid: Id) -> Option<Kept<In_sidebar>> {
    let mut arena = Box::new(Arena::new());
    let mut rec: Box<In_sidebar> = Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
    let ok = with_db(c, |db| unsafe { etch_load_sidebar(ctx, db, uid, &mut *rec, &mut *arena) });
    ok.then_some(Kept { rec, _arena: arena })
}

/// A kept sidebar's bytes, in pieces between its splices: no reads.
pub fn sidebar_bytes(ctx: &Ctx, k: &Kept<In_sidebar>, v: &Viewer) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(8192);
    let (_bufs, ptrs) = tmps(TMPS_SIDEBAR);
    let mut splits = vec![];
    unsafe { etch_build_sidebar(ctx, &*k.rec, v, &mut out, ptrs.as_ptr(), &mut splits) };
    let mut pieces = vec![];
    let mut at = 0;
    for s in splits {
        pieces.push(out[at..s].to_vec());
        at = s;
    }
    pieces.push(out[at..].to_vec());
    pieces
}

/// A message's variants from a forwarded record (the action filled it): no reads.
pub fn message_from(ctx: &Ctx, rec: *const In_message, viewers: &[&Viewer]) -> (Vec<Vec<u8>>, Vec<u8>) {
    let (_bufs, ptrs) = tmps(TMPS_MESSAGE);
    let mut splits = vec![];
    let mut keys: Vec<i64> = vec![];
    let mut variants = vec![];
    let mut by = Vec::with_capacity(viewers.len());
    for v in viewers {
        let k = unsafe { etch_bits_message(rec, *v) };
        let i = match keys.iter().position(|x| *x == k) {
            Some(i) => i,
            None => {
                keys.push(k);
                let mut out = Vec::with_capacity(9000);
                unsafe { etch_build_message(ctx, rec, *v, &mut out, ptrs.as_ptr(), &mut splits) };
                variants.push(out);
                keys.len() - 1
            }
        };
        by.push(i as u8);
    }
    (variants, by)
}

/// post_message, as the generated rule (etch_act_post_message), inside the writer's transaction.
pub fn post_message(c: &Connection, ctx: &Ctx, w: &mut etch_rt::abi::Writer, viewer: Id, vrec: &Viewer, room: Id, body: &str, d: &mut etch_rt::abi::Deltas) -> Result<(), &'static str> {
    with(c, |db, _arena| unsafe {
        let args = Args_post_message { room, body: Str::of(body) };
        let r = etch_act_post_message(ctx, db, w, viewer, vrec, &args, d);
        if r == 0 {
            Ok(())
        } else {
            Err(OUTCOMES_POST_MESSAGE[r as usize])
        }
    })
}
'''
open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('ok')
