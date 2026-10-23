//! The HTTP/1.1 server: one event loop per worker thread, each with its own listening socket on
//! the same port (SO_REUSEPORT, so the kernel spreads connections), sharing nothing with the
//! others. A request is parsed in place in the connection's read buffer and answered into the
//! connection's `Out`, which is sent with scatter writes. Buffers are made once per connection.
//!
//! Only what the fast path needs is parsed: the request line, Host, Cookie, Content-Type and
//! Content-Length. Request bodies are read whole into the buffer (at most `MAX_REQUEST` bytes).

use crate::out::Out;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};
use std::io::{self, Read};
use std::net::SocketAddr;
use std::sync::Arc;

/// The largest request (headers and body) a connection reads.
pub const MAX_REQUEST: usize = 64 * 1024;

pub struct Req<'a> {
    pub method: &'a [u8],
    /// the path, without the query
    pub path: &'a [u8],
    /// the query, without the `?` (empty when there is none)
    pub query: &'a [u8],
    pub host: &'a [u8],
    pub cookie: &'a [u8],
    pub content_type: &'a [u8],
    /// the page the request came from (an action goes back to it)
    pub referer: &'a [u8],
    pub body: &'a [u8],
    /// Accept-Encoding names gzip
    pub gzip: bool,
    /// a reconnecting live stream's last event (Last-Event-ID; 0 when there is none)
    pub last_event_id: u64,
    /// the worker's index, for per-worker state the handler keeps
    pub worker: usize,
}

impl<'a> Req<'a> {
    /// The value of cookie `name`, if the request has it.
    pub fn cookie(&self, name: &[u8]) -> Option<&'a [u8]> {
        let mut rest = self.cookie;
        while !rest.is_empty() {
            let end = rest.iter().position(|&c| c == b';').unwrap_or(rest.len());
            let mut pair = &rest[..end];
            while let [b' ', tail @ ..] = pair {
                pair = tail;
            }
            if pair.len() > name.len() && pair.starts_with(name) && pair[name.len()] == b'=' {
                return Some(&pair[name.len() + 1..]);
            }
            rest = if end < rest.len() { &rest[end + 1..] } else { &[] };
        }
        None
    }
}

pub trait Handler: Send + Sync + 'static {
    /// Answers one request into `out` (status line, headers and body).
    fn handle(&self, req: &Req, out: &mut Out);
    /// A live stream (`GET /_burin/live`), if the request is one the viewer may have: its topics
    /// and first bytes. None answers it with `handle` (a refusal).
    fn live(&self, _req: &Req) -> Option<Live> {
        None
    }
    /// A live stream ended (the browser left the page, or it was closed).
    fn live_closed(&self, _live: &Live) {}
}

/// A live stream's subscription.
pub struct Live {
    pub topics: Vec<u64>,
    /// who is watching (closed if a topic's audience drops them)
    pub viewer: i64,
    /// the status line, headers and the events it missed, sent first
    pub head: Vec<u8>,
    /// the last event `head` holds (or the page's): later ones are sent once each
    pub last: u64,
    /// what the app keeps about the stream (a screen and key it watches)
    pub tag: (usize, i64),
}

const LISTENER: Token = Token(usize::MAX);
const WAKE: Token = Token(usize::MAX - 1);
/// A live stream's heartbeat (a comment line): idle proxies don't close it, and a dead peer shows.
const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);
/// The most a live stream may have waiting to be sent: a reader that far behind is closed.
const MAX_BEHIND: usize = 1 << 20;

struct Conn {
    tok: Token,
    stream: TcpStream,
    buf: Box<[u8]>,
    len: usize,
    out: Out,
    writing: bool,
    /// a live stream: its subscription and what waits to be sent
    live: Option<Box<(Live, Vec<u8>)>>,
}

enum Next {
    Keep,
    Close,
    /// the request became a live stream
    Live(Live),
}

/// The read workers' nice value. The writer stays at 0: when both want a CPU, the writer gets
/// about nine times a reader's share (CFS weights 1024 against 110); reads still use every
/// cycle the writer doesn't.
pub const READER_NICE: i32 = 10;

/// Serves `handler` on `addr` with `workers` threads; doesn't return unless binding fails.
pub fn serve<H: Handler>(addr: SocketAddr, workers: usize, handler: Arc<H>) -> io::Result<()> {
    let mut threads = vec![];
    for w in 0..workers {
        let listener = bind(addr)?;
        let h = handler.clone();
        threads.push(std::thread::Builder::new().name(format!("burin-w{w}")).spawn(move || {
            // readers yield to the writer: a write is never kept waiting behind reads
            crate::lower_priority(READER_NICE);
            if let Err(e) = run(w, listener, h) {
                eprintln!("worker {w}: {e}");
            }
        })?);
    }
    for t in threads {
        let _ = t.join();
    }
    Ok(())
}

fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    use socket2::{Domain, Socket, Type};
    let s = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
    s.set_reuse_address(true)?;
    #[cfg(unix)]
    s.set_reuse_port(true)?;
    s.set_nonblocking(true)?;
    s.bind(&addr.into())?;
    s.listen(1024)?;
    Ok(TcpListener::from_std(s.into()))
}

fn run<H: Handler>(worker: usize, mut listener: TcpListener, h: Arc<H>) -> io::Result<()> {
    let mut poll = Poll::new()?;
    poll.registry().register(&mut listener, LISTENER, Interest::READABLE)?;
    let inbox = crate::live::HUB.register(Arc::new(mio::Waker::new(poll.registry(), WAKE)?));
    let mut events = Events::with_capacity(256);
    let mut conns: Vec<Option<Conn>> = vec![];
    let mut free: Vec<usize> = vec![];
    // live streams by topic
    let mut subs: std::collections::HashMap<u64, Vec<usize>> = std::collections::HashMap::new();
    let mut beat = std::time::Instant::now();
    loop {
        if let Err(e) = poll.poll(&mut events, Some(HEARTBEAT)) {
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        let mut closing: Vec<usize> = vec![];
        for ev in events.iter() {
            if ev.token() == LISTENER {
                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_nodelay(true);
                            let i = free.pop().unwrap_or_else(|| {
                                conns.push(None);
                                conns.len() - 1
                            });
                            poll.registry().register(&mut stream, Token(i), Interest::READABLE)?;
                            conns[i] = Some(Conn { tok: Token(i), stream, buf: vec![0u8; MAX_REQUEST].into_boxed_slice(), len: 0, out: Out::default(), writing: false, live: None });
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
                continue;
            }
            if ev.token() == WAKE {
                // what the writer published: to this worker's streams on each topic
                let pushes = std::mem::take(&mut *inbox.lock().unwrap());
                for p in pushes {
                    match p {
                        crate::live::Push::Event(e) => {
                            for &i in subs.get(&e.topic).map(|v| v.as_slice()).unwrap_or(&[]) {
                                if let Some(Some(c)) = conns.get_mut(i) {
                                    let Some(l) = c.live.as_mut() else { continue };
                                    if e.id <= l.0.last {
                                        continue;
                                    }
                                    l.0.last = e.id;
                                    if !e.is_for(l.0.viewer) {
                                        continue;
                                    }
                                    l.1.extend_from_slice(&e.bytes);
                                    if !flush_live(c, &poll) {
                                        closing.push(i);
                                    }
                                }
                            }
                        }
                        crate::live::Push::Revoke { topic, audience } => {
                            for &i in subs.get(&topic).map(|v| v.as_slice()).unwrap_or(&[]) {
                                if let Some(Some(Conn { live: Some(l), .. })) = conns.get(i) {
                                    if audience.binary_search(&l.0.viewer).is_err() {
                                        closing.push(i);
                                    }
                                }
                            }
                        }
                    }
                }
                continue;
            }
            let i = ev.token().0;
            let next = match conns.get_mut(i).and_then(|c| c.as_mut()) {
                Some(c) if c.live.is_some() => {
                    // a live stream: what's waiting goes out; anything the browser sends is dropped
                    let mut open = flush_live(c, &poll);
                    loop {
                        match c.stream.read(&mut c.buf[..]) {
                            Ok(0) => {
                                open = false;
                                break;
                            }
                            Ok(_) => {}
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                            Err(_) => {
                                open = false;
                                break;
                            }
                        }
                    }
                    if open { Next::Keep } else { Next::Close }
                }
                Some(c) => drive(worker, c, &*h, &poll),
                None => continue,
            };
            match next {
                Next::Keep => {}
                Next::Close => closing.push(i),
                Next::Live(mut live) => {
                    let c = conns[i].as_mut().unwrap();
                    for t in &live.topics {
                        subs.entry(*t).or_default().push(i);
                    }
                    let head = std::mem::take(&mut live.head);
                    c.live = Some(Box::new((live, head)));
                    if !flush_live(c, &poll) {
                        closing.push(i);
                    }
                }
            }
        }
        // heartbeats
        if beat.elapsed() >= HEARTBEAT {
            beat = std::time::Instant::now();
            for (i, c) in conns.iter_mut().enumerate() {
                if let Some(c) = c.as_mut().filter(|c| c.live.is_some()) {
                    c.live.as_mut().unwrap().1.extend_from_slice(b":\n\n");
                    if !flush_live(c, &poll) {
                        closing.push(i);
                    }
                }
            }
        }
        closing.sort_unstable();
        closing.dedup();
        for i in closing {
            if let Some(mut c) = conns[i].take() {
                let _ = poll.registry().deregister(&mut c.stream);
                if let Some(l) = c.live.take() {
                    for t in &l.0.topics {
                        if let Some(v) = subs.get_mut(t) {
                            v.retain(|&k| k != i);
                            if v.is_empty() {
                                subs.remove(t);
                            }
                        }
                    }
                    h.live_closed(&l.0);
                }
                free.push(i);
            }
        }
    }
}

/// Sends what a live stream has waiting, as far as the socket takes it now; false when it's
/// done (the peer went away, or it's too far behind).
fn flush_live(c: &mut Conn, poll: &Poll) -> bool {
    use std::io::Write;
    let Some(l) = c.live.as_mut() else { return false };
    let pending = &mut l.1;
    let mut sent = 0;
    while sent < pending.len() {
        match c.stream.write(&pending[sent..]) {
            Ok(0) => return false,
            Ok(n) => sent += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    pending.drain(..sent);
    if pending.len() > MAX_BEHIND {
        return false;
    }
    let interest = if pending.is_empty() { Interest::READABLE } else { Interest::READABLE | Interest::WRITABLE };
    let _ = poll.registry().reregister(&mut c.stream, c.tok, interest);
    true
}

/// Reads, answers and writes until the connection would block; false when it's done.
fn drive<H: Handler>(worker: usize, c: &mut Conn, h: &H, poll: &Poll) -> Next {
    match drive_requests(worker, c, h, poll) {
        Ok(true) => Next::Keep,
        Ok(false) => Next::Close,
        Err(live) => Next::Live(live),
    }
}

/// `drive`'s requests; Err: the request became a live stream.
fn drive_requests<H: Handler>(worker: usize, c: &mut Conn, h: &H, poll: &Poll) -> Result<bool, Live> {
    loop {
        if c.writing {
            match c.out.send(&mut c.stream) {
                Ok(true) => {
                    c.writing = false;
                    let close = c.out.close;
                    c.out.reset();
                    if close {
                        return Ok(false);
                    }
                    let _ = poll.registry().reregister(&mut c.stream, c.tok, Interest::READABLE);
                }
                Ok(false) => return Ok(true), // waits for WRITABLE
                Err(_) => return Ok(false),
            }
        }
        // a whole request in the buffer?
        match parse(&c.buf[..c.len]) {
            Parsed::Done(used, req) => {
                let req = Req { worker, ..req };
                if req.method == b"GET" && req.path == b"/_burin/live" {
                    if let Some(live) = h.live(&req) {
                        c.len = 0;
                        return Err(live);
                    }
                }
                c.out.reset();
                h.handle(&req, &mut c.out);
                // the request's bytes are used; what follows (a pipelined request) moves up
                c.buf.copy_within(used..c.len, 0);
                c.len -= used;
                c.writing = true;
                match c.out.send(&mut c.stream) {
                    Ok(true) => {
                        c.writing = false;
                        if c.out.close {
                            return Ok(false);
                        }
                        c.out.reset();
                    }
                    Ok(false) => {
                        let _ = poll.registry().reregister(&mut c.stream, c.tok, Interest::WRITABLE);
                        return Ok(true);
                    }
                    Err(_) => return Ok(false),
                }
            }
            Parsed::Bad => {
                c.out.reset();
                c.out.push_static(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                c.out.close = true;
                c.writing = true;
                continue;
            }
            Parsed::Partial => {
                if c.len == c.buf.len() {
                    c.out.reset();
                    c.out.push_static(b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    c.out.close = true;
                    c.writing = true;
                    continue;
                }
                match c.stream.read(&mut c.buf[c.len..]) {
                    Ok(0) => return Ok(false),
                    Ok(n) => c.len += n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(true),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => return Ok(false),
                }
            }
        }
    }
}

enum Parsed<'a> {
    Done(usize, Req<'a>),
    Partial,
    Bad,
}

fn parse(buf: &[u8]) -> Parsed<'_> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut r = httparse::Request::new(&mut headers);
    let head = match r.parse(buf) {
        Ok(httparse::Status::Complete(n)) => n,
        Ok(httparse::Status::Partial) => return Parsed::Partial,
        Err(_) => return Parsed::Bad,
    };
    let (method, target) = match (r.method, r.path) {
        (Some(m), Some(p)) => (m.as_bytes(), p.as_bytes()),
        _ => return Parsed::Bad,
    };
    let (mut host, mut cookie, mut ctype, mut referer, mut clen): (&[u8], &[u8], &[u8], &[u8], usize) = (b"", b"", b"", b"", 0);
    let mut gzip = false;
    let mut last_event_id = 0u64;
    for hd in r.headers.iter() {
        let n = hd.name.as_bytes();
        if n.eq_ignore_ascii_case(b"host") {
            host = hd.value;
        } else if n.eq_ignore_ascii_case(b"cookie") {
            cookie = hd.value;
        } else if n.eq_ignore_ascii_case(b"referer") {
            referer = hd.value;
        } else if n.eq_ignore_ascii_case(b"content-type") {
            ctype = hd.value;
        } else if n.eq_ignore_ascii_case(b"content-length") {
            match std::str::from_utf8(hd.value).ok().and_then(|s| s.trim().parse::<usize>().ok()) {
                Some(v) => clen = v,
                None => return Parsed::Bad,
            }
        } else if n.eq_ignore_ascii_case(b"last-event-id") {
            last_event_id = std::str::from_utf8(hd.value).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        } else if n.eq_ignore_ascii_case(b"accept-encoding") {
            gzip = hd.value.windows(4).any(|w| w.eq_ignore_ascii_case(b"gzip"));
        } else if n.eq_ignore_ascii_case(b"transfer-encoding") {
            return Parsed::Bad; // chunked bodies aren't taken
        }
    }
    if head + clen > MAX_REQUEST {
        return Parsed::Bad;
    }
    if buf.len() < head + clen {
        return Parsed::Partial;
    }
    let (path, query) = match target.iter().position(|&c| c == b'?') {
        Some(q) => (&target[..q], &target[q + 1..]),
        None => (target, &target[target.len()..]),
    };
    Parsed::Done(head + clen, Req { method, path, query, host, cookie, content_type: ctype, referer, body: &buf[head..head + clen], gzip, last_event_id, worker: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_request_and_its_cookie() {
        let raw = b"GET /rooms/1?at=2 HTTP/1.1\r\nHost: x\r\nCookie: a=1; session_token=abc; b=2\r\n\r\nGET /";
        match parse(raw) {
            Parsed::Done(n, r) => {
                assert_eq!(r.path, b"/rooms/1");
                assert_eq!(r.query, b"at=2");
                assert_eq!(r.cookie(b"session_token"), Some(&b"abc"[..]));
                assert_eq!(r.cookie(b"b"), Some(&b"2"[..]));
                assert_eq!(r.cookie(b"c"), None);
                assert_eq!(&raw[n..], b"GET /");
            }
            _ => panic!("not parsed"),
        }
    }

    #[test]
    fn waits_for_the_body() {
        let raw = b"POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\nab";
        assert!(matches!(parse(raw), Parsed::Partial));
    }
}
