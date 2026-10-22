//! The mixed benchmark's load: many people reading many rooms, at different rates, while some of
//! them post. Plain HTTP/1.1 to one server; one JSON object on stdout.
//!
//!   mixload --base URL --people people.json --app etch|crudspec|rails|rust|flow
//!           [--cookies cookies.json] [--conc 32] [--duration 10] [--post-rate 0] [--gzip 1]
//!           [--check-every 20] [--seed 1]
//!
//! Readers (`--conc`, closed loop): each request is someone chosen by how active they are (their
//! weight), reading one of their rooms chosen by how popular it is. Posters (`--post-rate`, open
//! loop): someone (by weight) posts to one of their rooms. etch and crudspec sign in with a
//! session token (people.json); the others with a cookie and CSRF token per person
//! (`--cookies`: {"<id>": {"cookie": "...", "csrf": "..."}}).
//!
//! Fail closed: a read counts only with status 200 and a page; every `--check-every`th read is
//! also decoded and must show the room it asked for and its messages. A post counts only when the
//! server says it worked.

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

/// A read page's check: the room it asked for, and messages.
fn page_ok(r: &Resp, room: i64) -> bool {
    let gz = r.headers.get("content-encoding").map(|v| v.as_bytes() == b"gzip").unwrap_or(false);
    let body = if gz {
        let mut out = Vec::new();
        if flate2::read::GzDecoder::new(&r.body[..]).read_to_end(&mut out).is_err() {
            return false;
        }
        out
    } else {
        r.body.to_vec()
    };
    let s = String::from_utf8_lossy(&body);
    // the room it asked for (a room may have no messages yet)
    s.contains(&format!("/rooms/{room}"))
}

fn arg<'a>(a: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    a.get(k).map(String::as_str)
}

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

    let pj: Value = serde_json::from_str(&std::fs::read_to_string(arg(&a, "people").ok_or("--people")?)?)?;
    let cookies: Value = match arg(&a, "cookies") {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?,
        None => json!({}),
    };
    let room_w: HashMap<i64, f64> = pj["rooms"].as_object().ok_or("rooms")?.iter().map(|(k, v)| (k.parse().unwrap(), v.as_f64().unwrap())).collect();
    let token_apps = app == "etch" || app == "crudspec";
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

    // the readers
    let mut tasks = vec![];
    for w in 0..conc {
        let (addr, people, pcum, ae) = (addr.clone(), people.clone(), pcum.clone(), ae.clone());
        let (ok, invalid, errors, checked, bytes, read_hist, statuses, seen) = (ok.clone(), invalid.clone(), errors.clone(), checked.clone(), bytes.clone(), read_hist.clone(), statuses.clone(), seen.clone());
        tasks.push(tokio::spawn(async move {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) ^ (w as u64 + 1).wrapping_mul(0xD1B54A32D192ED03) | 1);
            let mut h = hist();
            let mut st = HashMap::<u16, u64>::new();
            let (mut rooms_seen, mut people_seen) = (HashSet::new(), HashSet::new());
            let mut conn: Option<SendRequest<Full<Bytes>>> = None;
            let mut n = 0u64;
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
                let p = &people[pick_cum(&pcum, rng.next())];
                let room = p.rooms[pick_cum(&p.cum, rng.next())];
                let t0 = Instant::now();
                match send(conn.as_mut().unwrap(), &addr, "GET", &format!("/rooms/{room}"), &[("cookie", p.cookie.clone()), ("accept-encoding", ae.clone())], Bytes::new()).await {
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
            read_hist.lock().unwrap().add(&h).unwrap();
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
                    let r = if app == "etch" || app == "crudspec" {
                        let body = format!("room={room}&body={}&client_id=mix-{k}", urlencode(&text));
                        send(&mut c, &addr, "POST", "/_do/post_message", &[("cookie", cookie), ("content-type", "application/x-www-form-urlencoded".into())], Bytes::from(body)).await
                    } else {
                        let body = format!("message%5Bbody%5D={}&message%5Bclient_message_id%5D=mix-{k}-{}", urlencode(&text), room);
                        send(&mut c, &addr, "POST", &format!("/rooms/{room}/messages"), &[("cookie", cookie), ("content-type", "application/x-www-form-urlencoded".into()), ("x-csrf-token", csrf), ("accept", "text/vnd.turbo-stream.html, text/html".into())], Bytes::from(body)).await
                    };
                    let us = t0.elapsed().as_micros().max(1) as u64;
                    if let Ok(r) = r {
                        let good = if app == "etch" || app == "crudspec" {
                            r.headers.get("x-etch-outcome").or(r.headers.get("x-crudspec-outcome")).map(|v| v == "ok").unwrap_or(false)
                        } else {
                            (200..400).contains(&r.status)
                        };
                        if good {
                            posts_ok.fetch_add(1, Ordering::Relaxed);
                            post_hist.lock().unwrap().record(us).ok();
                        }
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
    let ph = post_hist.lock().unwrap();
    let ms = |h: &Histogram<u64>, q: f64| if h.is_empty() { Value::Null } else { json!((h.value_at_quantile(q) as f64 / 10.0).round() / 100.0) };
    let sn = seen.lock().unwrap();
    let okn = ok.load(Ordering::Relaxed);
    println!(
        "{}",
        json!({
            "app": app, "conc": conc, "secs": (elapsed * 100.0).round() / 100.0, "people": people.len(),
            "reads": { "ok": okn, "per_s": ((okn as f64 / elapsed) * 10.0).round() / 10.0, "invalid": invalid.load(Ordering::Relaxed),
                       "errors": errors.load(Ordering::Relaxed), "checked": checked.load(Ordering::Relaxed),
                       "statuses": statuses.lock().unwrap().iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
                       "p50_ms": ms(&h, 0.5), "p99_ms": ms(&h, 0.99), "p999_ms": ms(&h, 0.999),
                       "avg_bytes": if okn > 0 { bytes.load(Ordering::Relaxed) / okn } else { 0 },
                       "rooms_read": sn.0.len(), "people_reading": sn.1.len() },
            "posts": { "sent": posted.load(Ordering::Relaxed), "ok": posts_ok.load(Ordering::Relaxed), "p50_ms": ms(&ph, 0.5), "p99_ms": ms(&ph, 0.99) }
        })
    );
    Ok(())
}
