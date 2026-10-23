//! etch's runtime: the prebuilt base every generated app links. Like a chip's base layers, it is
//! the same for every app; the compiler generates only the app's own layer.

pub mod abi;
pub mod app;
pub mod files;
pub mod ext;
pub mod gz;
pub mod html;
pub mod keys;
pub mod http;
pub mod out;
pub mod pass;
pub mod sign;
pub mod store;
pub mod time;

pub use http::{serve, Handler, Req};
pub use out::{push_int, Out};

/// Lowers the calling thread's scheduling priority to `nice` (Linux: per thread; no privilege
/// needed to lower it). Elsewhere, nothing.
pub fn lower_priority(nice: i32) {
    #[cfg(target_os = "linux")]
    unsafe {
        // with `who` 0, PRIO_PROCESS names the calling thread on Linux
        libc::setpriority(libc::PRIO_PROCESS, 0, nice);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = nice;
}

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
            // formatted into the cached bytes themselves: no String (a request crossing into a
            // new second allocated one)
            use std::io::Write;
            let cache = &mut *c;
            let mut w = &mut cache.1[..];
            let _ = write!(w, "{}", httpdate::HttpDate::from(now));
            cache.0 = secs;
        }
        b.extend_from_slice(&c.1);
    });
}

#[cfg(test)]
mod date_tests {
    #[test]
    fn the_date_header_is_httpdates_format() {
        let mut b = vec![];
        super::http_date(&mut b);
        let s = String::from_utf8(b).unwrap();
        assert_eq!(s.len(), 29);
        assert!(httpdate::parse_http_date(&s).is_ok(), "{s}");
        assert!(s.ends_with(" GMT"), "{s}");
    }
}
