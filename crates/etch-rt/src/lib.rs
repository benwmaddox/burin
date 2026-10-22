//! etch's runtime: the prebuilt base every generated app links. Like a chip's base layers, it is
//! the same for every app; the compiler generates only the app's own layer.

pub mod html;
pub mod http;
pub mod out;
pub mod sign;
pub mod time;

pub use http::{serve, Handler, Req};
pub use out::{push_int, Out};

/// The `Date` header's value for now, made at most once a second per thread.
pub fn http_date(b: &mut Vec<u8>) {
    use std::cell::RefCell;
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local! {
        static CACHE: RefCell<(u64, [u8; 29])> = const { RefCell::new((u64::MAX, [0; 29])) };
    }
    let now = SystemTime::now();
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != secs {
            let s = httpdate::fmt_http_date(now);
            c.1.copy_from_slice(&s.as_bytes()[..29]);
            c.0 = secs;
        }
        b.extend_from_slice(&c.1);
    });
}
