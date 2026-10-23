//! A response, as a list of byte ranges (segments) sent with one scatter write.
//!
//! A segment is either borrowed (bytes that outlive the response: `'static` constants, or bytes
//! owned by the snapshot the response holds) or a range of the response's own scratch buffer
//! (the few bytes made per request: a length, a date). Nothing is copied into one buffer before
//! it is sent; the kernel gathers the ranges.
//!
//! An `Out` belongs to a connection and is reused for every response on it, so once its vectors
//! have grown to a page's size, sending a page allocates nothing.

use std::any::Any;
use std::io::{self, IoSlice, Write};
use std::sync::Arc;

#[derive(Clone, Copy)]
enum Seg {
    /// bytes owned by something that outlives the response (see `Out::borrowed`)
    Ext(*const u8, usize),
    /// a range of `scratch`
    Own(u32, u32),
}

pub struct Out {
    segs: Vec<Seg>,
    scratch: Vec<u8>,
    hold: Option<Arc<dyn Any + Send + Sync>>,
    // the send's position: the segment, and the offset in it
    at: usize,
    off: usize,
    len: usize,
    /// close the connection once this response is sent
    pub close: bool,
}

// The raw pointers in `segs` point into `'static` data or into `hold`, which the `Out` keeps
// alive; neither is tied to a thread.
unsafe impl Send for Out {}

impl Default for Out {
    fn default() -> Self {
        Out { segs: Vec::with_capacity(128), scratch: Vec::with_capacity(4096), hold: None, at: 0, off: 0, len: 0, close: false }
    }
}

impl Out {
    /// Starts a new response (keeps the vectors' capacity).
    pub fn reset(&mut self) {
        self.segs.clear();
        self.scratch.clear();
        self.hold = None;
        self.at = 0;
        self.off = 0;
        self.len = 0;
        self.close = false;
    }

    /// Keeps `h` alive until the response is sent: the borrowed segments point into it.
    pub fn hold(&mut self, h: Arc<dyn Any + Send + Sync>) {
        self.hold = Some(h);
    }

    pub fn push_static(&mut self, b: &'static [u8]) {
        if !b.is_empty() {
            self.segs.push(Seg::Ext(b.as_ptr(), b.len()));
            self.len += b.len();
        }
    }

    /// Adds bytes owned by what `hold` was given.
    ///
    /// # Safety
    /// `b` must stay valid until the response is sent: it must be `'static`, or be owned by the
    /// value passed to `hold` (which must not be replaced afterwards).
    pub unsafe fn push_borrowed(&mut self, b: &[u8]) {
        if !b.is_empty() {
            self.segs.push(Seg::Ext(b.as_ptr(), b.len()));
            self.len += b.len();
        }
    }

    /// Adds bytes written by `f` into the response's own buffer.
    pub fn push_own(&mut self, f: impl FnOnce(&mut Vec<u8>)) {
        let start = self.scratch.len();
        f(&mut self.scratch);
        let n = self.scratch.len() - start;
        if n > 0 {
            self.segs.push(Seg::Own(start as u32, n as u32));
            self.len += n;
        }
    }

    /// The number of bytes added so far.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reserves a segment to be filled in later (a header that depends on the body's length):
    /// returns its index for `fill`.
    pub fn placeholder(&mut self) -> usize {
        self.segs.push(Seg::Own(0, 0));
        self.segs.len() - 1
    }

    /// Fills the segment `placeholder` reserved.
    pub fn fill(&mut self, at: usize, f: impl FnOnce(&mut Vec<u8>)) {
        let start = self.scratch.len();
        f(&mut self.scratch);
        let n = self.scratch.len() - start;
        self.segs[at] = Seg::Own(start as u32, n as u32);
        self.len += n;
    }

    pub fn done(&self) -> bool {
        self.at >= self.segs.len()
    }

    /// Sends as much as the socket takes; Ok(true) when all of it is sent.
    pub fn send(&mut self, w: &mut impl Write) -> io::Result<bool> {
        const MAX: usize = 64;
        while self.at < self.segs.len() {
            let mut bufs: [IoSlice; MAX] = [IoSlice::new(&[]); MAX];
            let mut n = 0;
            let mut i = self.at;
            while i < self.segs.len() && n < MAX {
                let s = self.slice(i);
                let s = if i == self.at { &s[self.off..] } else { s };
                bufs[n] = IoSlice::new(s);
                n += 1;
                i += 1;
            }
            let mut wrote = match w.write_vectored(&bufs[..n]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(k) => k,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            // move the position past what was written
            while wrote > 0 {
                let left = self.slice(self.at).len() - self.off;
                if wrote >= left {
                    wrote -= left;
                    self.at += 1;
                    self.off = 0;
                } else {
                    self.off += wrote;
                    wrote = 0;
                }
            }
        }
        Ok(true)
    }

    fn slice(&self, i: usize) -> &[u8] {
        match self.segs[i] {
            // Safety: see `push_borrowed`: the bytes outlive the response
            Seg::Ext(p, n) => unsafe { std::slice::from_raw_parts(p, n) },
            Seg::Own(s, n) => &self.scratch[s as usize..(s + n) as usize],
        }
    }

    /// The whole response as one buffer (tests and the golden-model check only).
    pub fn to_vec(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.len);
        for i in 0..self.segs.len() {
            v.extend_from_slice(self.slice(i));
        }
        v
    }

    /// The number of segments (the compiler's reports and tests).
    pub fn segments(&self) -> usize {
        self.segs.len()
    }

    /// The bytes this response made itself, rather than borrowed: what it wrote or copied.
    pub fn own_bytes(&self) -> usize {
        self.scratch.len()
    }
}

/// Writes n in decimal, with a sign when negative.
pub fn push_int_signed(b: &mut Vec<u8>, n: i64) {
    if n < 0 {
        b.push(b'-');
    }
    push_int(b, n.unsigned_abs());
}

/// Writes n in decimal.
pub fn push_int(b: &mut Vec<u8>, n: u64) {
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    let mut n = n;
    loop {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    b.extend_from_slice(&tmp[i..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_in_order_and_partial_writes() {
        let mut o = Out::default();
        let held: Arc<Vec<u8>> = Arc::new(b"held".to_vec());
        let p = o.placeholder();
        o.push_static(b"<a>");
        unsafe { o.push_borrowed(&held[..]) };
        o.hold(held.clone());
        o.push_own(|b| push_int(b, 1234));
        let n = o.len();
        o.fill(p, |b| push_int(b, n as u64));
        assert_eq!(o.to_vec(), b"11<a>held1234");
        // a writer that takes 3 bytes at a time
        struct Slow(Vec<u8>);
        impl Write for Slow {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                let k = b.len().min(3);
                self.0.extend_from_slice(&b[..k]);
                Ok(k)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut w = Slow(vec![]);
        while !o.send(&mut w).unwrap() {}
        assert_eq!(w.0, b"11<a>held1234");
    }
}
