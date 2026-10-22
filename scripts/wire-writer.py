# One-off: the writer runs the generated action and applies the delta edges it reports.
D = 'D:/code/etch/crates/campfire-etch/src/'
p = D + 'writer.rs'
s = open(p, encoding='utf-8').read()
a = s.index('pub fn run(mut c: Connection')
new_run = r'''pub fn run(mut c: Connection, ctx: Arc<Ctx>, snap: Arc<ArcSwap<Snap>>, rx: Receiver<Cmd>, stats: Arc<Stats>) {
    let seq: i64 = c
        .query_row(
            "SELECT max(s) FROM (SELECT max(seq) s FROM Message UNION ALL SELECT max(seq) FROM Room UNION ALL SELECT max(seq) FROM Membership UNION ALL SELECT max(seq) FROM Boost UNION ALL SELECT max(seq) FROM \"User\" UNION ALL SELECT max(seq) FROM Session)",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .expect("max seq")
        .unwrap_or(0);
    let mut w = Writer::new(seq);
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
                    let r = gen::post_message(&tx, &ctx.abi, &mut w, actor, room, &body, &mut deltas);
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
        let mut rebuild: HashSet<Id> = HashSet::new();
        let mut touched_rooms: HashSet<Id> = HashSet::new();
        for &(op, a, key, item) in &deltas.list {
            let reg = |a: i64| gen::REGISTERS.get(a as usize).copied().unwrap_or("");
            match op {
                // a new row's register: built where a screen list shows it (op 2)
                1 => {}
                2 => {
                    // appended to the room's messages: its fragment for the room's members
                    touched_rooms.insert(key);
                    if let Some(view) = next.rooms.get(&key).cloned() {
                        let frag = match state::build_msgs(&c, &ctx, &[item], &view.members).pop() {
                            Some(f) => Arc::new(f),
                            None => continue,
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
                    if next.sidebars.contains_key(&key) {
                        rebuild.insert(key);
                    }
                }
                4 if reg(a) == "sidebar" => {
                    rebuild.extend(next.sidebars.keys().copied());
                }
                // anything else (not reported by today's actions): let the rooms rebuild on demand
                _ => next.rooms.clear(),
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
        let _ = &mut c;
    }
}
'''
s = s[:a] + new_run
s = s.replace('use crate::db::{self, Id};', 'use crate::db::{self, Id};\nuse crate::gen;\nuse etch_rt::abi::{Deltas, Writer};')
s = s.replace('use rusqlite::{params, Connection};', 'use rusqlite::Connection;')
open(p, 'w', encoding='utf-8', newline='\n').write(s)

# the generated action, called with the writer's connection
g = D + 'gen.rs'
s = open(g, encoding='utf-8').read()
s += r'''
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
'''
open(g, 'w', encoding='utf-8', newline='\n').write(s)
print('ok')
