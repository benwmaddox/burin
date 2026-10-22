# One-off: precompressed pieces in campfire-etch (every page piece raw and gzip, built at write time).
D = 'D:/code/etch/crates/'

def edit(f, old, new, count=1):
    p = D + f
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == count, (f, old[:80], s.count(old))
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)

# ---- the request says whether it takes gzip
edit('etch-rt/src/http.rs', '''    pub body: &'a [u8],
    /// the worker's index''', '''    pub body: &'a [u8],
    /// Accept-Encoding names gzip
    pub gzip: bool,
    /// the worker's index''')
edit('etch-rt/src/http.rs', '''    let (mut host, mut cookie, mut ctype, mut clen): (&[u8], &[u8], &[u8], usize) = (b"", b"", b"", 0);''',
     '''    let (mut host, mut cookie, mut ctype, mut clen): (&[u8], &[u8], &[u8], usize) = (b"", b"", b"", 0);
    let mut gzip = false;''')
edit('etch-rt/src/http.rs', '''        } else if n.eq_ignore_ascii_case(b"transfer-encoding") {''', '''        } else if n.eq_ignore_ascii_case(b"accept-encoding") {
            gzip = hd.value.windows(4).any(|w| w.eq_ignore_ascii_case(b"gzip"));
        } else if n.eq_ignore_ascii_case(b"transfer-encoding") {''')
edit('etch-rt/src/http.rs', '''content_type: ctype, body: &buf[head..head + clen], worker: 0 })''', '''content_type: ctype, body: &buf[head..head + clen], gzip, worker: 0 })''')
edit('campfire-hand/tests/hot_path.rs', '''content_type: b"", body: b"", worker: 0 };''', '''content_type: b"", body: b"", gzip: false, worker: 0 };''')

# ---- the snapshot's pieces: raw and precompressed
S = 'campfire-etch/src/state.rs'
edit(S, '''pub struct RoomView {
    pub id: Id,
    /// the members' user ids, sorted: a viewer's place picks each message's variant
    pub members: Vec<Id>,
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    pub tok: RoomTokens,''', '''/// A piece of a page, as it is and precompressed (built once, at write time).
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
    pub tok_seg: Arc<Seg>,''')
edit(S, '''    pub variants: Vec<Vec<u8>>,''', '''    pub variants: Vec<Seg>,''')
edit(S, '''    pub pieces: Vec<Vec<u8>>,''', '''    pub pieces: Vec<Seg>,''')
edit(S, '''    pub end: Arc<Vec<u8>>,''', '''    pub end: Arc<Seg>,''')
edit(S, '''                variants.push(gen::message(&ctx.abi, &data, &v));''', '''                variants.push(Seg::new(gen::message(&ctx.abi, &data, &v)));''')
edit(S, '''    Ok(Some(RoomView { id: room_id, members, head, tail, tok, headers, msgs: frags }))''',
     '''    let tok_seg = Arc::new(Seg::new(tok.plain.as_bytes().to_vec()));
    Ok(Some(RoomView { id: room_id, members, head: Arc::new(Seg::new(head)), tail: Arc::new(Seg::new(tail)), tok, tok_seg, headers, msgs: frags }))''')
edit(S, '''    Ok(Sidebar { pieces: render::sidebar(ctx, &side_data(c, users, uid)?) })''',
     '''    Ok(Sidebar { pieces: render::sidebar(ctx, &side_data(c, users, uid)?).into_iter().map(Seg::new).collect() })''')
edit(S, '''pub fn assemble<'a>(room: &'a RoomView, place: usize, side: &'a Sidebar, end: &'a [u8], mut push: impl FnMut(&'a [u8])) {
    push(&room.head);
    for m in &room.msgs {
        push(&m.variants[m.by_member[place] as usize]);
    }
    push(&room.tail);
    for (i, p) in side.pieces.iter().enumerate() {
        if i > 0 {
            push(room.tok.plain.as_bytes());
        }
        push(p);
    }
    push(end);
}''', '''pub fn assemble<'a>(room: &'a RoomView, place: usize, side: &'a Sidebar, end: &'a Seg, mut push: impl FnMut(&'a Seg)) {
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
}''')

# ---- the writer rebuilds a room view sharing its pieces
edit('campfire-etch/src/writer.rs', '''                    head: view.head.clone(),
                    tail: view.tail.clone(),''', '''                    head: view.head.clone(),
                    tail: view.tail.clone(),
                    tok_seg: view.tok_seg.clone(),''')
edit('campfire-etch/src/main.rs', '''        end: Arc::new(render::page_end(&ctx)),''', '''        end: Arc::new(state::Seg::new(render::page_end(&ctx))),''')

# ---- the fast path: raw, or the precompressed pieces joined
V = 'campfire-etch/src/serve.rs'
edit(V, '''        unsafe { out.push_borrowed(&room.headers) };
        out.push_own(|b| {''', '''        unsafe { out.push_borrowed(&room.headers) };
        if req.gzip {
            out.push_static(b"Content-Encoding: gzip\\r\\n");
        }
        out.push_own(|b| {''')
edit(V, '''        let head_len = out.len();
        state::assemble(room, place, side, &snap.end, |b| unsafe { out.push_borrowed(b) });
        let body_len = out.len() - head_len;''', '''        let head_len = out.len();
        if req.gzip {
            // the pieces' deflate bytes joined; the checksum combined from theirs
            out.push_static(&etch_rt::gz::HEADER);
            let mut crc = etch_rt::gz::Crc::default();
            state::assemble(room, place, side, &snap.end, |s| {
                unsafe { out.push_borrowed(&s.gz.deflate) };
                crc.add(&s.gz);
            });
            out.push_static(&etch_rt::gz::FINAL);
            out.push_own(|b| b.extend_from_slice(&crc.trailer()));
        } else {
            state::assemble(room, place, side, &snap.end, |s| unsafe { out.push_borrowed(&s.raw) });
        }
        let body_len = out.len() - head_len;''')
edit(V, '''            self.check(&snap, uid, rid, req.host, out, body_len);''', '''            self.check(&snap, uid, rid, req.host, out, body_len, req.gzip);''')
edit(V, '''    fn check(&self, snap: &Snap, uid: Id, rid: Id, host: &[u8], out: &Out, body_len: usize) {
        let all = out.to_vec();
        let fast = &all[all.len() - body_len..];''', '''    fn check(&self, snap: &Snap, uid: Id, rid: Id, host: &[u8], out: &Out, body_len: usize, gzip: bool) {
        let all = out.to_vec();
        let mut fast = all[all.len() - body_len..].to_vec();
        if gzip {
            use std::io::Read;
            let mut plain = vec![];
            if flate2::read::GzDecoder::new(&fast[..]).read_to_end(&mut plain).is_err() {
                plain = b"(not valid gzip)".to_vec();
            }
            fast = plain;
        }
        let fast = &fast[..];''')
edit(V, '''    state::assemble(&room, place, &side, &end, |b| page.extend_from_slice(b));''', '''    let end = state::Seg::new(end);
    state::assemble(&room, place, &side, &end, |s| page.extend_from_slice(&s.raw));''')
print('ok')
