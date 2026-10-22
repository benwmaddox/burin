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
}

const LISTENER: Token = Token(usize::MAX);

struct Conn {
    tok: Token,
    stream: TcpStream,
    buf: Box<[u8]>,
    len: usize,
    out: Out,
    writing: bool,
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
        threads.push(std::thread::Builder::new().name(format!("etch-w{w}")).spawn(move || {
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
    let mut events = Events::with_capacity(256);
    let mut conns: Vec<Option<Conn>> = vec![];
    let mut free: Vec<usize> = vec![];
    loop {
        if let Err(e) = poll.poll(&mut events, None) {
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
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
                            conns[i] = Some(Conn { tok: Token(i), stream, buf: vec![0u8; MAX_REQUEST].into_boxed_slice(), len: 0, out: Out::default(), writing: false });
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
                continue;
            }
            let i = ev.token().0;
            let keep = match conns.get_mut(i).and_then(|c| c.as_mut()) {
                Some(c) => drive(worker, c, &*h, &poll),
                None => continue,
            };
            if !keep {
                if let Some(mut c) = conns[i].take() {
                    let _ = poll.registry().deregister(&mut c.stream);
                }
                free.push(i);
            }
        }
    }
}

/// Reads, answers and writes until the connection would block; false when it's done.
fn drive<H: Handler>(worker: usize, c: &mut Conn, h: &H, poll: &Poll) -> bool {
    loop {
        if c.writing {
            match c.out.send(&mut c.stream) {
                Ok(true) => {
                    c.writing = false;
                    let close = c.out.close;
                    c.out.reset();
                    if close {
                        return false;
                    }
                    let _ = poll.registry().reregister(&mut c.stream, c.tok, Interest::READABLE);
                }
                Ok(false) => return true, // waits for WRITABLE
                Err(_) => return false,
            }
        }
        // a whole request in the buffer?
        match parse(&c.buf[..c.len]) {
            Parsed::Done(used, req) => {
                let req = Req { worker, ..req };
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
                            return false;
                        }
                        c.out.reset();
                    }
                    Ok(false) => {
                        let _ = poll.registry().reregister(&mut c.stream, c.tok, Interest::WRITABLE);
                        return true;
                    }
                    Err(_) => return false,
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
                    Ok(0) => return false,
                    Ok(n) => c.len += n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return true,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => return false,
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
    Parsed::Done(head + clen, Req { method, path, query, host, cookie, content_type: ctype, referer, body: &buf[head..head + clen], gzip, worker: 0 })
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
