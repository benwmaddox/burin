// Notes' Rust functions (apps/notes/notes.burin), with the C ABI an `extern fn` has.

use burin_rt::ext::{text, write, Out};
use std::sync::atomic::{AtomicI64, Ordering};

/// The text in capitals (Unicode's rules).
#[no_mangle]
pub unsafe extern "C" fn shout(p: *const u8, n: i64, out: *mut Out) {
    write(out, &text(p, n).to_uppercase());
}

/// The next number of a counter this process keeps: an effect (`extern effect fn`), so only an
/// action calls it, on the writer.
#[no_mangle]
pub extern "C" fn next_serial() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
