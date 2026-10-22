//! Requests. The fast path answers a member's room page from the snapshot: three map lookups,
//! one binary search, then a list of byte ranges (no allocation once the connection's buffers
//! have grown, no copying, no escaping or formatting). Everything else goes to the cold path,
//! which reads SQLite as an ordinary server does.

use crate::db::{self, Id};
use crate::render::Ctx;
use crate::state::{self, Snap};
use crate::writer::{self, Cmd, Stats};
use arc_swap::ArcSwap;
use etch_rt::{push_int, Out, Req};
use rusqlite::Connection;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

pub struct App {
    pub ctx: Arc<Ctx>,
    pub snap: Arc<ArcSwap<Snap>>,
    pub writer: SyncSender<Cmd>,
    pub db_path: String,
    /// test mode: every fast page is compared with a fresh render from SQLite
    pub verify: bool,
    pub counts: Counts,
    pub wstats: Arc<Stats>,
}

#[derive(Default)]
pub struct Counts {
    pub fast: AtomicU64,
    pub cold: AtomicU64,
    pub verified: AtomicU64,
    pub mismatched: AtomicU64,
    pub refused: AtomicU64,
}

thread_local! {
    static READER: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

impl App {
    fn with_reader<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        READER.with(|r| {
            let mut r = r.borrow_mut();
            if r.is_none() {
                *r = Some(db::open(&self.db_path, false).expect("open the database to read"));
            }
            f(r.as_ref().unwrap())
        })
    }

    /// The fast path; false when the request isn't one it answers.
    pub fn fast(&self, req: &Req, out: &mut Out) -> bool {
        if req.method != b"GET" || !req.query.is_empty() || req.cookie(b"flash").is_some() {
            return false;
        }
        let rid = match room_path(req.path) {
            Some(r) => r,
            None => return false,
        };
        let token = match req.cookie(b"session_token") {
            Some(t) => t,
            None => return false,
        };
        let snap = self.snap.load_full();
        if req.host != snap.host.as_bytes() {
            return false;
        }
        let (uid, room, side) = match (snap.sessions.get(token), snap.rooms.get(&rid)) {
            (Some(u), Some(r)) => match snap.sidebars.get(u) {
                Some(s) => (*u, r, s),
                None => {
                    writer::send(&self.writer, Cmd::WarmSidebar(*u));
                    return false;
                }
            },
            (Some(u), None) => {
                writer::send(&self.writer, Cmd::WarmRoom(rid));
                if !snap.sidebars.contains_key(u) {
                    writer::send(&self.writer, Cmd::WarmSidebar(*u));
                }
                return false;
            }
            _ => return false,
        };
        let place = match room.members.binary_search(&uid) {
            Ok(p) => p,
            Err(_) => return false, // not a member: the cold path sends them home
        };
        // the headers: the length goes in once the body is known
        let first = out.placeholder();
        // Safety: every borrowed range is owned by `snap`, which the response holds
        unsafe { out.push_borrowed(&room.headers) };
        if req.gzip {
            out.push_static(b"Content-Encoding: gzip\r\n");
        }
        out.push_own(|b| {
            b.extend_from_slice(b"Date: ");
            etch_rt::http_date(b);
            b.extend_from_slice(b"\r\n\r\n");
        });
        let head_len = out.len();
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
        let body_len = out.len() - head_len;
        out.fill(first, |b| {
            b.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: ");
            push_int(b, body_len as u64);
            b.extend_from_slice(b"\r\n");
        });
        if self.verify {
            self.check(&snap, uid, rid, req.host, out, body_len, req.gzip);
        }
        out.hold(snap);
        self.counts.fast.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// The golden-model check: the fast page must equal a fresh render from SQLite.
    fn check(&self, snap: &Snap, uid: Id, rid: Id, host: &[u8], out: &Out, body_len: usize, gzip: bool) {
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
        let fast = &fast[..];
        let fresh = self.with_reader(|c| render_page(c, &self.ctx, &String::from_utf8_lossy(host), &snap.users, uid, rid));
        self.counts.verified.fetch_add(1, Ordering::Relaxed);
        match fresh {
            Ok(Some(f)) if f == fast => {}
            other => {
                let n = self.counts.mismatched.fetch_add(1, Ordering::Relaxed);
                if n < 5 {
                    let at = match &other {
                        Ok(Some(f)) => f.iter().zip(fast).position(|(a, b)| a != b).unwrap_or(f.len().min(fast.len())),
                        _ => 0,
                    };
                    let ctx = |b: &[u8]| String::from_utf8_lossy(&b[at.saturating_sub(60)..(at + 60).min(b.len())]).into_owned();
                    match &other {
                        Ok(Some(f)) => eprintln!("verify: room {rid} user {uid}: differs at byte {at}\n fast:  {}\n fresh: {}", ctx(fast), ctx(f)),
                        _ => eprintln!("verify: room {rid} user {uid}: no fresh render"),
                    }
                }
            }
        }
    }

    fn cold(&self, req: &Req, out: &mut Out) {
        self.counts.cold.fetch_add(1, Ordering::Relaxed);
        let host = String::from_utf8_lossy(req.host).into_owned();
        match (req.method, req.path) {
            (b"GET", b"/up") => text(out, 200, "ok"),
            (b"GET", b"/_etch/stats") => {
                let c = &self.counts;
                let w = &self.wstats;
                let s = format!(
                    r#"{{"fast":{},"cold":{},"verified":{},"mismatched":{},"refused":{},"batches":{},"posts":{},"rebuilt_sidebars":{},"version":{},"write_reads":{},"forwarded":{},"patched":{}}}"#,
                    c.fast.load(Ordering::Relaxed), c.cold.load(Ordering::Relaxed), c.verified.load(Ordering::Relaxed),
                    c.mismatched.load(Ordering::Relaxed), c.refused.load(Ordering::Relaxed), w.batches.load(Ordering::Relaxed),
                    w.posts.load(Ordering::Relaxed), w.rebuilt_sidebars.load(Ordering::Relaxed), self.snap.load().version,
                    w.reads.load(Ordering::Relaxed), w.forwarded.load(Ordering::Relaxed), w.patched.load(Ordering::Relaxed)
                );
                respond(out, 200, "application/json", &[], s.as_bytes());
            }
            (b"POST", b"/_do/post_message") => self.post(req, out),
            (b"GET", p) if room_path(p).is_some() => {
                let rid = room_path(p).unwrap();
                let token = req.cookie(b"session_token").map(|t| String::from_utf8_lossy(t).into_owned());
                let snap = self.snap.load_full();
                let r = self.with_reader(|c| -> rusqlite::Result<Result<Vec<u8>, &'static str>> {
                    let uid = match &token {
                        Some(t) => db::session_user(c, t)?,
                        None => None,
                    };
                    let uid = match uid {
                        Some(u) => u,
                        None => return Ok(Err("/session/new")),
                    };
                    Ok(render_page(c, &self.ctx, &host, &snap.users, uid, rid)?.ok_or("/"))
                });
                match r {
                    Ok(Ok(body)) => {
                        let cookie = format!("last_room={rid}; Path=/; Max-Age=31536000");
                        respond(out, 200, "text/html; charset=utf-8", &[("Set-Cookie", &cookie), ("Vary", "Accept-Encoding")], &body);
                    }
                    Ok(Err(to)) => redirect(out, to, None),
                    Err(e) => text(out, 500, &e.to_string()),
                }
            }
            _ => text(out, 404, "not found"),
        }
    }

    /// POST /_do/post_message (room, body): handed to the writer; answered once committed.
    fn post(&self, req: &Req, out: &mut Out) {
        let form = parse_form(req.body);
        let field = |n: &str| form.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
        let room: Option<Id> = field("room").or_else(|| field("__arg_room")).and_then(|r| r.trim_start_matches('#').parse().ok());
        let body = field("body").unwrap_or_default();
        let actor = req.cookie(b"session_token").and_then(|t| self.snap.load().sessions.get(t).copied());
        let actor = match actor {
            Some(a) => a,
            None => {
                let t = req.cookie(b"session_token").map(|t| String::from_utf8_lossy(t).into_owned());
                match t.and_then(|t| self.with_reader(|c| db::session_user(c, &t).ok().flatten())) {
                    Some(a) => a,
                    None => return outcome(out, "unauthenticated", "/session/new"),
                }
            }
        };
        let room = match room {
            Some(r) => r,
            None => return outcome(out, "invalid", "/"),
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        if !writer::send(&self.writer, Cmd::Post { actor, room, body, reply: tx }) {
            self.counts.refused.fetch_add(1, Ordering::Relaxed);
            return text(out, 503, "busy");
        }
        match rx.recv() {
            Ok(Ok(_)) => outcome(out, "ok", &format!("/rooms/{room}")),
            Ok(Err(e)) => outcome(out, e, &format!("/rooms/{room}")),
            Err(_) => text(out, 500, "writer stopped"),
        }
    }
}

impl etch_rt::Handler for App {
    fn handle(&self, req: &Req, out: &mut Out) {
        if !self.fast(req, out) {
            out.reset();
            self.cold(req, out);
        }
    }
}

/// The room page for this user, fresh from SQLite (the cold path, and the golden model the fast
/// path is checked against); None when they aren't a member or the room isn't one phase 1a renders.
pub fn render_page(c: &Connection, ctx: &Ctx, host: &str, users: &std::collections::HashMap<Id, db::User>, uid: Id, rid: Id) -> rusqlite::Result<Option<Vec<u8>>> {
    let room = match state::build_room(c, ctx, host, users, rid)? {
        Some(r) => r,
        None => return Ok(None),
    };
    let place = match room.members.binary_search(&uid) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let side = state::build_sidebar(c, ctx, users, uid)?;
    let end = crate::render::page_end(ctx);
    let mut page = Vec::with_capacity(400_000);
    let end = state::Seg::new(end);
    state::assemble(&room, place, &side, &end, |s| page.extend_from_slice(&s.raw));
    Ok(Some(page))
}

/// "/rooms/<id>" exactly
fn room_path(p: &[u8]) -> Option<Id> {
    let rest = p.strip_prefix(b"/rooms/")?;
    if rest.is_empty() || rest.len() > 18 || !rest.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    std::str::from_utf8(rest).ok()?.parse().ok()
}

fn respond(out: &mut Out, status: u16, ctype: &str, headers: &[(&str, &str)], body: &[u8]) {
    let reason = match status {
        200 => "OK",
        303 => "See Other",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "",
    };
    let mut h = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: {ctype}\r\n", body.len());
    for (k, v) in headers {
        h.push_str(k);
        h.push_str(": ");
        h.push_str(v);
        h.push_str("\r\n");
    }
    out.push_own(|b| {
        b.extend_from_slice(h.as_bytes());
        b.extend_from_slice(b"Date: ");
        etch_rt::http_date(b);
        b.extend_from_slice(b"\r\n\r\n");
        b.extend_from_slice(body);
    });
}

fn text(out: &mut Out, status: u16, s: &str) {
    respond(out, status, "text/plain; charset=utf-8", &[], s.as_bytes());
}

fn redirect(out: &mut Out, to: &str, outcome: Option<&str>) {
    let mut hs = vec![("Location", to)];
    if let Some(o) = outcome {
        hs.push(("X-Crudspec-Outcome", o));
    }
    out.push_own(|b| {
        b.extend_from_slice(b"HTTP/1.1 303 See Other\r\nContent-Length: 0\r\n");
        for (k, v) in &hs {
            b.extend_from_slice(k.as_bytes());
            b.extend_from_slice(b": ");
            b.extend_from_slice(v.as_bytes());
            b.extend_from_slice(b"\r\n");
        }
        b.extend_from_slice(b"\r\n");
    });
}

fn outcome(out: &mut Out, o: &str, to: &str) {
    redirect(out, to, Some(o));
}

/// application/x-www-form-urlencoded
fn parse_form(body: &[u8]) -> Vec<(String, String)> {
    body.split(|&c| c == b'&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = match p.iter().position(|&c| c == b'=') {
                Some(i) => (&p[..i], &p[i + 1..]),
                None => (p, &p[p.len()..]),
            };
            (unescape(k), unescape(v))
        })
        .collect()
}

fn unescape(s: &[u8]) -> String {
    let mut o = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'+' => o.push(b' '),
            b'%' if i + 2 < s.len() => {
                let h = std::str::from_utf8(&s[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(v) => {
                        o.push(v);
                        i += 2;
                    }
                    None => o.push(b'%'),
                }
            }
            c => o.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&o).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_and_paths() {
        let f = parse_form(b"room=486777696&body=benchmark+message+1%21&client_id=x");
        assert_eq!(f[1], ("body".to_string(), "benchmark message 1!".to_string()));
        assert_eq!(room_path(b"/rooms/42"), Some(42));
        assert_eq!(room_path(b"/rooms/42/edit"), None);
    }
}
