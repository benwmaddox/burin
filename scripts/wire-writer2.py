# One-off: the writer forwards (kept viewer and sidebar records, forwarded message records, patches).
D = 'D:/code/etch/crates/campfire-etch/src/'
p = D + 'writer.rs'
s = open(p, encoding='utf-8').read()
s = s.replace('''pub struct Stats {
    pub batches: AtomicU64,
    pub posts: AtomicU64,
    pub rebuilt_sidebars: AtomicU64,
}''', '''pub struct Stats {
    pub batches: AtomicU64,
    pub posts: AtomicU64,
    pub rebuilt_sidebars: AtomicU64,
    /// registers loaded from SQLite after a commit (a read after the write)
    pub reads: AtomicU64,
    /// registers built from records the action forwarded, or from patched kept records: no read
    pub forwarded: AtomicU64,
    pub patched: AtomicU64,
}''')
a = s.index('pub fn run(mut c: Connection')
s = s[:a] + r'''pub fn run(mut c: Connection, ctx: Arc<Ctx>, snap: Arc<ArcSwap<Snap>>, rx: Receiver<Cmd>, stats: Arc<Stats>) {
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
        for &(reg, key, item, list, field, value) in &deltas.patches {
            if let Some(k) = sides.get_mut(&key) {
                if unsafe { gen::patch(reg, list, field, &mut *k.rec as *mut gen::In_sidebar as *mut u8, item, value) } {
                    dirty.insert(key);
                } else {
                    reload.insert(key);
                }
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
'''
s = s.replace('use std::collections::HashSet;', 'use std::collections::{HashMap, HashSet};')
open(p, 'w', encoding='utf-8', newline='\n').write(s)

p = D + 'serve.rs'
s = open(p, encoding='utf-8').read()
old = '''r#"{{"fast":{},"cold":{},"verified":{},"mismatched":{},"refused":{},"batches":{},"posts":{},"rebuilt_sidebars":{},"version":{}}}"#,'''
assert old in s
s = s.replace(old, '''r#"{{"fast":{},"cold":{},"verified":{},"mismatched":{},"refused":{},"batches":{},"posts":{},"rebuilt_sidebars":{},"version":{},"write_reads":{},"forwarded":{},"patched":{}}}"#,''')
old = '''w.posts.load(Ordering::Relaxed), w.rebuilt_sidebars.load(Ordering::Relaxed), self.snap.load().version'''
assert old in s
s = s.replace(old, '''w.posts.load(Ordering::Relaxed), w.rebuilt_sidebars.load(Ordering::Relaxed), self.snap.load().version,
                    w.reads.load(Ordering::Relaxed), w.forwarded.load(Ordering::Relaxed), w.patched.load(Ordering::Relaxed)''')
open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('ok')
