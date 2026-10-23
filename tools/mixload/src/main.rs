//! The mixed benchmark's load: many people reading many rooms, at different rates, while some of
//! them post, each acting as a browser does. Plain HTTP/1.1 to one server; one JSON object on
//! stdout.
//!
//!   mixload --base URL --people people.json --app burin|crudspec|rails|rust|flow
//!           [--cookies cookies.json] [--conc 32] [--duration 10] [--post-rate 0] [--gzip 1]
//!           [--check-every 20] [--seed 1] [--visits 20] [--sidebar-reload 0.05] [--sidebar PATH|none]
//!
//! Readers (`--conc`, closed loop) are browser sessions, one after another: someone chosen by how
//! active they are (their weight) opens the app at one of their rooms (chosen by how popular it
//! is): the page, then the sidebar's frame where the app has one (Rails' /users/me/sidebar, asked
//! with Turbo-Frame: user_sidebar; crudspec builds its sidebar into every page). Then they visit
//! rooms (`--visits` pages a session on average): a Turbo visit asks for the page only, the frame
//! is kept; after a visit the sidebar is reloaded with probability `--sidebar-reload` (Rails
//! reloads it when the tab is shown again). burin's page says when the room was unread for the
//! viewer, and its script marks it read: so does this. Static files are never asked for (a
//! browser keeps them).
//!
//! Posters (`--post-rate`, open loop): someone (by weight) posts to one of their rooms, as the
//! browser's form does: burin and crudspec answer with a redirect, which the browser follows (the
//! post's time includes that page); Rails' composer asks for a Turbo Stream and gets no page.
//! burin and crudspec sign in with a session token (people.json); the others with a cookie and
//! CSRF token per person (`--cookies`: {"<id>": {"cookie": "...", "csrf": "..."}}).
//!
//! Fail closed: a read counts only with status 200 and a page; every `--check-every`th page is
//! also decoded and must show the room it asked for; every sidebar must be the frame with rooms
//! in it. A post counts only when the server says it worked (and the page it leads to loads).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use hdrhistogram::Histogram;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::SendRequest;
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::TcpStream;

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Person {
    id: i64,
    cookie: String,
    csrf: String,
    weight: f64,
    rooms: Vec<i64>,
    /// the rooms' cumulative popularity
    cum: Vec<f64>,
}

/// xorshift64*: each task its own stream
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn pick_cum(cum: &[f64], r: f64) -> usize {
    let x = r * cum[cum.len() - 1];
    cum.partition_point(|c| *c < x).min(cum.len() - 1)
}

async fn connect(addr: &str) -> Res<SendRequest<Full<Bytes>>> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(sender)
}

struct Resp {
    status: u16,
    headers: hyper::HeaderMap,
    body: Bytes,
}

async fn send(s: &mut SendRequest<Full<Bytes>>, addr: &str, method: &str, path: &str, headers: &[(&str, String)], body: Bytes) -> Res<Resp> {
    let mut req = hyper::Request::builder().method(method).uri(path).header("host", addr);
    for (k, v) in headers {
        req = req.header(*k, v.as_str());
    }
    let req = req.body(Full::new(body))?;
    tokio::time::timeout(Duration::from_secs(30), async {
        s.ready().await?;
        let r = s.send_request(req).await?;
        let status = r.status().as_u16();
        let headers = r.headers().clone();
        let body = r.into_body().collect().await?.to_bytes();
        Ok(Resp { status, headers, body })
    })
    .await
    .map_err(|_| "timed out after 30 s")?
}

fn urlencode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

fn hist() -> Histogram<u64> {
    Histogram::new_with_bounds(1, 60_000_000, 3).unwrap()
}

/// A response's text (gzip decoded; empty when it isn't valid gzip).
fn body_text(r: &Resp) -> String {
    let gz = r.headers.get("content-encoding").map(|v| v.as_bytes() == b"gzip").unwrap_or(false);
    if gz {
        let mut out = Vec::new();
        if flate2::read::GzDecoder::new(&r.body[..]).read_to_end(&mut out).is_err() {
            return String::new();
        }
        String::from_utf8_lossy(&out).into_owned()
    } else {
        String::from_utf8_lossy(&r.body).into_owned()
    }
}

/// A page's head: its first bytes, decoded (what a meta tag is looked for in; cheap, so the load
/// generator doesn't spend its cores decoding whole pages).
fn head_text(r: &Resp) -> String {
    const N: u64 = 8192;
    let gz = r.headers.get("content-encoding").map(|v| v.as_bytes() == b"gzip").unwrap_or(false);
    let mut out = Vec::new();
    if gz {
        let _ = flate2::read::GzDecoder::new(&r.body[..]).take(N).read_to_end(&mut out);
    } else {
        out.extend_from_slice(&r.body[..r.body.len().min(N as usize)]);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A read page's check: the room it asked for (a room may have no messages yet).
fn page_ok(r: &Resp, room: i64) -> bool {
    body_text(r).contains(&format!("/rooms/{room}"))
}

/// A sidebar frame's check: the frame, with the viewer's rooms in it.
fn sidebar_ok(r: &Resp) -> bool {
    let s = body_text(r);
    s.contains("user_sidebar") && s.contains("href=\"/rooms/")
}

/// A redirect's target, as a path on this server.
fn location(r: &Resp, addr: &str) -> Option<String> {
    let l = r.headers.get("location")?.to_str().ok()?;
    let l = l.strip_prefix("http://").map(|x| x.strip_prefix(addr).unwrap_or(x)).unwrap_or(l);
    l.starts_with('/').then(|| l.to_string())
}

fn arg<'a>(a: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    a.get(k).map(String::as_str)
}

const ACCEPT_PAGE: &str = "text/html, application/xhtml+xml";

#[tokio::main]
async fn main() -> Res<()> {
    let raw: Vec<String> = std::env::args().collect();
    let mut a = HashMap::new();
    let mut it = raw.iter().skip(1);
    while let Some(k) = it.next() {
        if let Some(k) = k.strip_prefix("--") {
            a.insert(k.to_string(), it.next().cloned().unwrap_or_default());
        }
    }
    let base = arg(&a, "base").ok_or("--base")?;
    let addr = base.trim_start_matches("http://").trim_end_matches('/').to_string();
    let app = arg(&a, "app").ok_or("--app")?.to_string();
    let conc: usize = arg(&a, "conc").and_then(|v| v.parse().ok()).unwrap_or(32);
    let secs: f64 = arg(&a, "duration").and_then(|v| v.parse().ok()).unwrap_or(10.0);
    let post_rate: f64 = arg(&a, "post-rate").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let gzip = arg(&a, "gzip").map(|v| v != "0").unwrap_or(true);
    let check_every: u64 = arg(&a, "check-every").and_then(|v| v.parse().ok()).unwrap_or(20);
    let seed: u64 = arg(&a, "seed").and_then(|v| v.parse().ok()).unwrap_or(1);
    let visits: f64 = arg(&a, "visits").and_then(|v| v.parse().ok()).unwrap_or(20.0);
    let sidebar_reload: f64 = arg(&a, "sidebar-reload").and_then(|v| v.parse().ok()).unwrap_or(0.05);
    // where the app's sidebar frame is (none: it's built into every page)
    let sidebar: Option<String> = match arg(&a, "sidebar") {
        Some("none") => None,
        Some(p) => Some(p.to_string()),
        None if app == "crudspec" => None,
        None => Some("/users/me/sidebar".to_string()),
    };

    let pj: Value = serde_json::from_str(&std::fs::read_to_string(arg(&a, "people").ok_or("--people")?)?)?;
    let cookies: Value = match arg(&a, "cookies") {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?,
        None => json!({}),
    };
    let room_w: HashMap<i64, f64> = pj["rooms"].as_object().ok_or("rooms")?.iter().map(|(k, v)| (k.parse().unwrap(), v.as_f64().unwrap())).collect();
    let token_apps = app == "burin" || app == "crudspec";
    let mut people = vec![];
    for p in pj["people"].as_array().ok_or("people")? {
        let id = p["id"].as_i64().unwrap();
        let (cookie, csrf) = if token_apps {
            (format!("session_token={}", p["token"].as_str().unwrap_or("")), String::new())
        } else {
            match cookies.get(id.to_string()) {
                Some(c) => (c["cookie"].as_str().unwrap_or("").to_string(), c["csrf"].as_str().unwrap_or("").to_string()),
                None => continue, // not signed in on this server
            }
        };
        let rooms: Vec<i64> = p["rooms"].as_array().unwrap().iter().map(|r| r.as_i64().unwrap()).collect();
        let mut cum = vec![];
        let mut t = 0.0;
        for r in &rooms {
            t += room_w.get(r).copied().unwrap_or(0.01);
            cum.push(t);
        }
        people.push(Person { id, cookie, csrf, weight: p["weight"].as_f64().unwrap_or(1.0), rooms, cum });
    }
    if people.is_empty() {
        return Err("nobody to sign in as".into());
    }
    let mut pcum = vec![];
    let mut t = 0.0;
    for p in &people {
        t += p.weight;
        pcum.push(t);
    }
    let people = Arc::new(people);
    let pcum = Arc::new(pcum);
    let ae = if gzip { "gzip" } else { "identity" }.to_string();

    let start = Instant::now();
    let deadline = start + Duration::from_secs_f64(secs);
    let (ok, invalid, errors, checked, bytes) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let read_hist = Arc::new(Mutex::new(hist()));
    let statuses = Arc::new(Mutex::new(HashMap::<u16, u64>::new()));
    let seen = Arc::new(Mutex::new((HashSet::<i64>::new(), HashSet::<i64>::new())));
    let (sb_ok, sb_bytes, marked, sessions) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let sb_hist = Arc::new(Mutex::new(hist()));

    // the readers: browser sessions, one after another
    let mut tasks = vec![];
    for w in 0..conc {
        let (addr, people, pcum, ae, sidebar, app) = (addr.clone(), people.clone(), pcum.clone(), ae.clone(), sidebar.clone(), app.clone());
        let (ok, invalid, errors, checked, bytes, read_hist, statuses, seen) = (ok.clone(), invalid.clone(), errors.clone(), checked.clone(), bytes.clone(), read_hist.clone(), statuses.clone(), seen.clone());
        let (sb_ok, sb_bytes, marked, sessions, sb_hist) = (sb_ok.clone(), sb_bytes.clone(), marked.clone(), sessions.clone(), sb_hist.clone());
        tasks.push(tokio::spawn(async move {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) ^ (w as u64 + 1).wrapping_mul(0xD1B54A32D192ED03) | 1);
            let (mut h, mut hs) = (hist(), hist());
            let mut st = HashMap::<u16, u64>::new();
            let (mut rooms_seen, mut people_seen) = (HashSet::new(), HashSet::new());
            let mut conn: Option<SendRequest<Full<Bytes>>> = None;
            let mut n = 0u64;
            // the session: whose, and how many pages are left in it (0: the next opens the app)
            let (mut who, mut left) = (0usize, 0u64);
            while Instant::now() < deadline {
                if conn.is_none() {
                    match connect(&addr).await {
                        Ok(c) => conn = Some(c),
                        Err(_) => {
                            errors.fetch_add(1, Ordering::Relaxed);
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            continue;
                        }
                    }
                }
                let opening = left == 0;
                if opening {
                    who = pick_cum(&pcum, rng.next());
                    // a geometric number of pages, `visits` on average, at least one
                    let q = 1.0 - 1.0 / visits.max(1.0);
                    left = if q <= 0.0 { 1 } else { 1 + (rng.next().max(1e-12).ln() / q.ln()).floor() as u64 };
                    sessions.fetch_add(1, Ordering::Relaxed);
                }
                let p = &people[who];
                let room = p.rooms[pick_cum(&p.cum, rng.next())];
                left -= 1;
                let t0 = Instant::now();
                match send(conn.as_mut().unwrap(), &addr, "GET", &format!("/rooms/{room}"), &[("cookie", p.cookie.clone()), ("accept-encoding", ae.clone()), ("accept", ACCEPT_PAGE.into())], Bytes::new()).await {
                    Ok(r) => {
                        h.record(t0.elapsed().as_micros().max(1) as u64).ok();
                        *st.entry(r.status).or_default() += 1;
                        n += 1;
                        let mut good = r.status == 200 && r.body.len() > 1000;
                        if good && n % check_every == 0 {
                            checked.fetch_add(1, Ordering::Relaxed);
                            good = page_ok(&r, room);
                        }
                        if good {
                            ok.fetch_add(1, Ordering::Relaxed);
                            bytes.fetch_add(r.body.len() as u64, Ordering::Relaxed);
                            rooms_seen.insert(room);
                            people_seen.insert(p.id);
                        } else {
                            invalid.fetch_add(1, Ordering::Relaxed);
                        }
                        let close = r.headers.get("connection").map(|v| v == "close").unwrap_or(false);
                        // burin's page script: the room was unread for the viewer, so it marks it read
                        if good && !close && app == "burin" && head_text(&r).contains("<meta name=\"current-room-unread\" content=\"true\">") {
                            let body = format!("room={room}");
                            let referer = format!("http://{addr}/rooms/{room}");
                            match send(conn.as_mut().unwrap(), &addr, "POST", "/_do/read_room", &[("cookie", p.cookie.clone()), ("content-type", "application/x-www-form-urlencoded".into()), ("referer", referer)], Bytes::from(body)).await {
                                Ok(rr) => {
                                    if rr.headers.get("x-burin-outcome").map(|v| v == "ok").unwrap_or(false) {
                                        marked.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(_) => {
                                    errors.fetch_add(1, Ordering::Relaxed);
                                    conn = None;
                                    continue;
                                }
                            }
                        }
                        if close {
                            conn = None;
                            continue;
                        }
                    }
                    Err(_) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                        conn = None;
                        continue;
                    }
                }
                // the sidebar's frame: when the app opens, and now and then after a visit
                if let Some(path) = &sidebar {
                    if opening || rng.next() < sidebar_reload {
                        let t1 = Instant::now();
                        match send(conn.as_mut().unwrap(), &addr, "GET", path, &[("cookie", p.cookie.clone()), ("accept-encoding", ae.clone()), ("accept", ACCEPT_PAGE.into()), ("turbo-frame", "user_sidebar".into())], Bytes::new()).await {
                            Ok(r) => {
                                hs.record(t1.elapsed().as_micros().max(1) as u64).ok();
                                *st.entry(r.status).or_default() += 1;
                                if r.status == 200 && sidebar_ok(&r) {
                                    sb_ok.fetch_add(1, Ordering::Relaxed);
                                    sb_bytes.fetch_add(r.body.len() as u64, Ordering::Relaxed);
                                } else {
                                    invalid.fetch_add(1, Ordering::Relaxed);
                                }
                                if r.headers.get("connection").map(|v| v == "close").unwrap_or(false) {
                                    conn = None;
                                }
                            }
                            Err(_) => {
                                errors.fetch_add(1, Ordering::Relaxed);
                                conn = None;
                            }
                        }
                    }
                }
            }
            read_hist.lock().unwrap().add(&h).unwrap();
            sb_hist.lock().unwrap().add(&hs).unwrap();
            let mut s = statuses.lock().unwrap();
            for (k, v) in st {
                *s.entry(k).or_default() += v;
            }
            let mut sn = seen.lock().unwrap();
            sn.0.extend(rooms_seen);
            sn.1.extend(people_seen);
        }));
    }

    // the posters: open loop, each post on its own connection
    let post_hist = Arc::new(Mutex::new(hist()));
    let (posted, posts_ok) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let poster = {
        let (addr, people, pcum, post_hist, posted, posts_ok, app) = (addr.clone(), people.clone(), pcum.clone(), post_hist.clone(), posted.clone(), posts_ok.clone(), app.clone());
        tokio::spawn(async move {
            if post_rate <= 0.0 {
                return;
            }
            let mut rng = Rng(seed ^ 0xA5A5_5A5A_1234_5678);
            let per = Duration::from_secs_f64(1.0 / post_rate);
            let mut next = Instant::now();
            let mut inflight = vec![];
            let mut k = 0u64;
            while next < deadline {
                tokio::time::sleep_until(next.into()).await;
                next += per;
                k += 1;
                let p = &people[pick_cum(&pcum, rng.next())];
                let room = p.rooms[pick_cum(&p.cum, rng.next())];
                let (cookie, csrf, app, addr) = (p.cookie.clone(), p.csrf.clone(), app.clone(), addr.clone());
                let (post_hist, posts_ok) = (post_hist.clone(), posts_ok.clone());
                posted.fetch_add(1, Ordering::Relaxed);
                let text = format!("mixed bench post {k} from {}", p.id);
                inflight.push(tokio::spawn(async move {
                    let t0 = Instant::now();
                    let Ok(mut c) = connect(&addr).await else { return };
                    let token_app = app == "burin" || app == "crudspec";
                    let r = if token_app {
                        let body = format!("room={room}&body={}&client_id=mix-{k}", urlencode(&text));
                        // from the room's page (the browser sends it as the referer)
                        let referer = format!("http://{addr}/rooms/{room}");
                        send(&mut c, &addr, "POST", "/_do/post_message", &[("cookie", cookie.clone()), ("content-type", "application/x-www-form-urlencoded".into()), ("accept", "text/vnd.turbo-stream.html, text/html, application/xhtml+xml".into()), ("referer", referer)], Bytes::from(body)).await
                    } else {
                        let body = format!("message%5Bbody%5D={}&message%5Bclient_message_id%5D=mix-{k}-{}", urlencode(&text), room);
                        send(&mut c, &addr, "POST", &format!("/rooms/{room}/messages"), &[("cookie", cookie.clone()), ("content-type", "application/x-www-form-urlencoded".into()), ("x-csrf-token", csrf), ("accept", "text/vnd.turbo-stream.html, text/html".into())], Bytes::from(body)).await
                    };
                    let good = match r {
                        Ok(r) if token_app => {
                            let said_ok = r.headers.get("x-burin-outcome").or(r.headers.get("x-crudspec-outcome")).map(|v| v == "ok").unwrap_or(false);
                            // the browser follows redirects to the page they lead to
                            let mut good = said_ok;
                            let mut at = r;
                            for _ in 0..3 {
                                if !good || !(300..400).contains(&at.status) {
                                    break;
                                }
                                let Some(to) = location(&at, &addr) else { good = false; break };
                                match send(&mut c, &addr, "GET", &to, &[("cookie", cookie.clone()), ("accept", ACCEPT_PAGE.into()), ("accept-encoding", "gzip".into())], Bytes::new()).await {
                                    Ok(next) => at = next,
                                    Err(_) => good = false,
                                }
                            }
                            good && at.status == 200
                        }
                        Ok(r) => (200..400).contains(&r.status),
                        Err(_) => false,
                    };
                    if good {
                        posts_ok.fetch_add(1, Ordering::Relaxed);
                        post_hist.lock().unwrap().record(t0.elapsed().as_micros().max(1) as u64).ok();
                    }
                }));
            }
            for t in inflight {
                let _ = t.await;
            }
        })
    };
    for t in tasks {
        t.await?;
    }
    poster.await?;
    let elapsed = start.elapsed().as_secs_f64();
    let h = read_hist.lock().unwrap();
    let sh = sb_hist.lock().unwrap();
    let ph = post_hist.lock().unwrap();
    let ms = |h: &Histogram<u64>, q: f64| if h.is_empty() { Value::Null } else { json!((h.value_at_quantile(q) as f64 / 10.0).round() / 100.0) };
    let sn = seen.lock().unwrap();
    let okn = ok.load(Ordering::Relaxed);
    let sbn = sb_ok.load(Ordering::Relaxed);
    println!(
        "{}",
        json!({
            "app": app, "conc": conc, "secs": (elapsed * 100.0).round() / 100.0, "people": people.len(),
            "model": { "visits": visits, "sidebar_reload": sidebar_reload, "sidebar": sidebar },
            "reads": { "ok": okn, "per_s": ((okn as f64 / elapsed) * 10.0).round() / 10.0, "invalid": invalid.load(Ordering::Relaxed),
                       "errors": errors.load(Ordering::Relaxed), "checked": checked.load(Ordering::Relaxed),
                       "statuses": statuses.lock().unwrap().iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
                       "p50_ms": ms(&h, 0.5), "p99_ms": ms(&h, 0.99), "p999_ms": ms(&h, 0.999),
                       "avg_bytes": if okn > 0 { bytes.load(Ordering::Relaxed) / okn } else { 0 },
                       "rooms_read": sn.0.len(), "people_reading": sn.1.len() },
            "sidebars": { "ok": sbn, "per_s": ((sbn as f64 / elapsed) * 10.0).round() / 10.0, "p50_ms": ms(&sh, 0.5), "p99_ms": ms(&sh, 0.99),
                          "avg_bytes": if sbn > 0 { sb_bytes.load(Ordering::Relaxed) / sbn } else { 0 } },
            "sessions": sessions.load(Ordering::Relaxed), "marked_read": marked.load(Ordering::Relaxed),
            "posts": { "sent": posted.load(Ordering::Relaxed), "ok": posts_ok.load(Ordering::Relaxed), "p50_ms": ms(&ph, 0.5), "p99_ms": ms(&ph, 0.99) }
        })
    );
    Ok(())
}
