p = '/mnt/d/code/etch/crates/etch-rt/src/app.rs'
s = open(p, encoding='utf-8').read()

def rep(old, new):
    global s
    assert old in s, old[:80]
    s = s.replace(old, new, 1)

rep('''    pub fn start(ctx: Arc<Ctx>, db_path: &str, host: &str, verify: bool, session_cookie: &'static [u8], seq: i64) -> Arc<App<G>> {''',
    '''    pub fn start(ctx: Arc<Ctx>, db_path: &str, host: &str, verify: bool, session_cookie: &'static [u8], seq: i64, scrub_ms: u64) -> Arc<App<G>> {''')
rep('''writer::<G>(w, ctx, snap, rx, stats, seq)).unwrap();''', '''writer::<G>(w, ctx, snap, rx, stats, seq, scrub_ms)).unwrap();''')
rep('''fn writer<G: Glue>(mut c: Connection, ctx: Arc<Ctx>, snap: Arc<ArcSwap<Snap>>, rx: Receiver<Cmd>, stats: Arc<Stats>, seq: i64) {''',
    '''fn writer<G: Glue>(mut c: Connection, ctx: Arc<Ctx>, snap: Arc<ArcSwap<Snap>>, rx: Receiver<Cmd>, stats: Arc<Stats>, seq: i64, scrub_ms: u64) {''')
rep('''    while let Ok(first) = rx.recv() {''', '''    // the scrubber's place in the kept pages
    let mut cursor = 0usize;
    loop {
        let first = if scrub_ms == 0 {
            match rx.recv() {
                Ok(c) => c,
                Err(_) => break,
            }
        } else {
            match rx.recv_timeout(std::time::Duration::from_millis(scrub_ms)) {
                Ok(c) => c,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    scrub::<G>(&c, &ctx, &snap, &mut viewers, &mut kept, &stats, &mut cursor);
                    continue;
                }
                Err(_) => break,
            }
        };''')
rep('''    let app = App::<G>::start(ctx, &db_path, &host, verify, session_cookie, seq);''', '''    let scrub_ms: u64 = arg("--scrub-ms").map(|v| v.parse().expect("--scrub-ms N")).unwrap_or(100);
    let app = App::<G>::start(ctx, &db_path, &host, verify, session_cookie, seq, scrub_ms);''')
rep('''    /// screen instances rebuilt because their audience changed
    pub audiences: AtomicU64,''', '''    /// screen instances rebuilt because their audience changed
    pub audiences: AtomicU64,
    /// kept pages the scrubber compared with a fresh build, and the ones it found different (and
    /// rebuilt): a missed edge
    pub scrubbed: AtomicU64,
    pub scrub_fixed: AtomicU64,''')
rep('''"audiences":{},"version":{}}}"#,''', '''"audiences":{},"scrubbed":{},"scrub_fixed":{},"version":{}}}"#,''')
rep('''g(&st.audiences), self.snap''', '''g(&st.audiences), g(&st.scrubbed), g(&st.scrub_fixed), self.snap''')

rep('''/// The audience a kept shared register is built for''', '''/// The scrubber (while the writer is idle): one kept page, for one member of its audience in
/// turn, is built afresh from SQLite (the viewer's record too) and compared with what readers are
/// served. A difference is a missed edge: logged, counted, and the page rebuilt.
fn scrub<G: Glue>(c: &Connection, ctx: &Ctx, snap: &ArcSwap<Snap>, viewers: &mut HashMap<i64, Rec>, kept: &mut HashMap<(usize, i64), Rec>, stats: &Stats, cursor: &mut usize) {
    let cur = snap.load_full();
    let mut all: Vec<(usize, i64)> = (0..G::SCREENS).flat_map(|s| cur.screens[s].keys().map(move |k| (s, *k))).collect();
    if all.is_empty() {
        return;
    }
    all.sort_unstable();
    let (s, key) = all[*cursor % all.len()];
    let round = *cursor / all.len();
    *cursor += 1;
    let inst = cur.screens[s][&key].clone();
    if inst.audience.is_empty() {
        return;
    }
    let place = round % inst.audience.len();
    let uid = inst.audience[place];
    // the page as served (a viewer whose own registers aren't kept yet: nothing to compare)
    let mut fast = Vec::with_capacity(400_000);
    if !G::assemble(&cur, s, key, uid, place, &mut |seg| fast.extend_from_slice(&seg.raw)) {
        return;
    }
    stats.scrubbed.fetch_add(1, Ordering::Relaxed);
    // the page built afresh
    let mut tmp = Snap::new(G::REGS, G::LISTS, G::SCREENS);
    let mut fresh_viewers = HashMap::new();
    let fresh = if build_screen::<G>(c, ctx, &mut tmp, &mut fresh_viewers, s, key) {
        tmp.screens[s].get(&key).cloned().and_then(|t| {
            let p = t.audience.binary_search(&uid).ok()?;
            fresh_viewers.entry(uid).or_insert_with(|| load_viewer::<G>(c, ctx, uid));
            for kind in G::viewer_regs(s) {
                let rec = load::<G>(c, ctx, kind, uid)?;
                tmp.regs[kind].insert(uid, Arc::new(build::<G>(ctx, kind, rec.ptr(), &[fresh_viewers[&uid].ptr()])));
            }
            let mut page = Vec::with_capacity(400_000);
            G::assemble(&tmp, s, key, uid, p, &mut |seg| page.extend_from_slice(&seg.raw)).then_some(page)
        })
    } else {
        None
    };
    if fresh.as_ref() == Some(&fast) {
        return;
    }
    stats.scrub_fixed.fetch_add(1, Ordering::Relaxed);
    let at = fresh.as_ref().map(|f| f.iter().zip(&fast).position(|(a, b)| a != b).unwrap_or(f.len().min(fast.len())));
    eprintln!("scrub: screen {s} key {key} viewer {uid}: the kept page differed from a fresh build (at byte {at:?}); rebuilt");
    // rebuilt: the screen, the viewer's record and own registers
    let mut next = cur.next();
    viewers.insert(uid, load_viewer::<G>(c, ctx, uid));
    if !build_screen::<G>(c, ctx, &mut next, viewers, s, key) {
        next.screens[s].remove(&key);
    }
    for kind in G::viewer_regs(s) {
        match load::<G>(c, ctx, kind, uid) {
            Some(rec) => {
                next.regs[kind].insert(uid, Arc::new(build::<G>(ctx, kind, rec.ptr(), &[viewers[&uid].ptr()])));
                kept.insert((kind, uid), rec);
            }
            None => {
                next.regs[kind].remove(&uid);
                kept.remove(&(kind, uid));
            }
        }
    }
    snap.store(Arc::new(next));
}

/// The audience a kept shared register is built for''')
open(p, 'w', encoding='utf-8').write(s)
print('ok')
