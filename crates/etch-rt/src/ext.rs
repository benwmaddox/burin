//! For external functions written in Rust (`apps/<app>/ext/ext.rs`): their arguments as `&str`,
//! their text results written out. A spec's
//!
//!     extern fn shout(t: Text): Text
//!
//! is, in Rust:
//!
//!     #[no_mangle]
//!     pub unsafe extern "C" fn shout(p: *const u8, n: i64, out: *mut etch_rt::ext::Out) {
//!         etch_rt::ext::write(out, &etch_rt::ext::text(p, n).to_uppercase());
//!     }
//!
//! (C functions use include/etch.h.) Numbers, flags, times and durations are `i64`.

/// A text result's buffer.
pub type Out = Vec<u8>;

/// A text argument (what isn't UTF-8 replaced, which allocates; valid text doesn't). # Safety:
/// `p` readable for `n` bytes during the call.
pub unsafe fn text<'a>(p: *const u8, n: i64) -> std::borrow::Cow<'a, str> {
    if p.is_null() || n <= 0 {
        return std::borrow::Cow::Borrowed("");
    }
    String::from_utf8_lossy(std::slice::from_raw_parts(p, n as usize))
}

/// Appends to a text result. # Safety: `out` the buffer the function was given.
pub unsafe fn write(out: *mut Out, s: &str) {
    (*out).extend_from_slice(s.as_bytes());
}
