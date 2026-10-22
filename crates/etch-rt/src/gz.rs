//! Precompressed pieces (DESIGN.md: data movement; the precomputed page is the normal path).
//!
//! Every piece of a page is compressed once, when it's built (at write time, so the best level is
//! affordable): a raw deflate stream flushed to a byte boundary, so pieces join into one valid
//! stream. Each piece also keeps its CRC-32 and the operator that shifts a CRC past it
//! (x^(8·len) mod P), so a page's checksum is combined from its pieces' in a few operations each,
//! never computed over the page. A read joins the pieces' bytes, adds an empty final block and the
//! gzip trailer: no compression and no checksum over the body per request.

use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::io::Write;

pub struct Piece {
    /// raw deflate, flushed to a byte boundary, not final
    pub deflate: Vec<u8>,
    /// the uncompressed bytes' CRC-32
    pub crc: u32,
    /// x^(8·len) mod P: multiplying a CRC by it moves the CRC past this piece
    pub op: u32,
    pub len: u64,
}

impl Piece {
    pub fn new(raw: &[u8]) -> Piece {
        let mut e = DeflateEncoder::new(Vec::with_capacity(raw.len() / 3 + 64), Compression::best());
        e.write_all(raw).expect("compress");
        e.flush().expect("flush"); // a sync flush: ends on a byte boundary
        let deflate = e.get_ref().clone();
        Piece { deflate, crc: crc32fast::hash(raw), op: x8n(raw.len() as u64), len: raw.len() as u64 }
    }
}

/// Deflate's header for a stored (uncompressed) block of `n` bytes (at most 65535), on a byte
/// boundary, not final: bytes made for one request join the stream with no compression work.
#[inline]
pub fn stored_header(n: usize) -> [u8; 5] {
    let n = n as u16;
    let [a, b] = n.to_le_bytes();
    let [c, d] = (!n).to_le_bytes();
    [0, a, b, c, d]
}

/// gzip's header: deflate, no name, no time, any OS.
pub const HEADER: [u8; 10] = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
/// an empty final block (fixed Huffman codes): it ends the joined stream
pub const FINAL: [u8; 2] = [0x03, 0x00];

/// A page's checksum, combined piece by piece.
#[derive(Clone, Copy, Default)]
pub struct Crc {
    pub crc: u32,
    pub len: u64,
}

impl Crc {
    #[inline]
    pub fn add(&mut self, p: &Piece) {
        self.crc = multmodp(p.op, self.crc) ^ p.crc;
        self.len += p.len;
    }
    /// Bytes made for this request, added as they are: the CRC continues over them.
    #[inline]
    pub fn add_raw(&mut self, b: &[u8]) {
        let mut h = crc32fast::Hasher::new_with_initial(self.crc);
        h.update(b);
        self.crc = h.finalize();
        self.len += b.len() as u64;
    }
    /// The gzip trailer: CRC-32 and length, little-endian.
    pub fn trailer(&self) -> [u8; 8] {
        let mut t = [0u8; 8];
        t[..4].copy_from_slice(&self.crc.to_le_bytes());
        t[4..].copy_from_slice(&(self.len as u32).to_le_bytes());
        t
    }
}

// CRC-32's polynomial arithmetic (reflected, as zlib's crc32_combine_op): a*b mod P
const POLY: u32 = 0xedb8_8320;

#[inline]
fn multmodp(a: u32, mut b: u32) -> u32 {
    let mut m: u32 = 1 << 31;
    let mut p: u32 = 0;
    loop {
        if a & m != 0 {
            p ^= b;
            if a & (m - 1) == 0 {
                break;
            }
        }
        m >>= 1;
        b = if b & 1 != 0 { (b >> 1) ^ POLY } else { b >> 1 };
    }
    p
}

/// x^(8·n) mod P
fn x8n(n: u64) -> u32 {
    // x^(2^k) mod P for k = 0..64, then the product over n·8's bits
    let mut pow = [0u32; 64];
    let mut p: u32 = 1 << 30; // x^1
    for slot in pow.iter_mut() {
        *slot = p;
        p = multmodp(p, p);
    }
    let mut r: u32 = 1 << 31; // x^0
    let mut k = n * 8;
    let mut i = 0;
    while k != 0 {
        if k & 1 != 0 {
            r = multmodp(pow[i], r);
        }
        k >>= 1;
        i += 1;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn joined_pieces_are_one_valid_gzip_stream() {
        let parts: Vec<&[u8]> = vec![b"<!DOCTYPE html><p>", b"hello, hello, hello world", b"", "é</p> and more text to compress".as_bytes()];
        let pieces: Vec<Piece> = parts.iter().map(|p| Piece::new(p)).collect();
        let mut gz = HEADER.to_vec();
        let mut crc = Crc::default();
        for p in &pieces {
            gz.extend_from_slice(&p.deflate);
            crc.add(p);
        }
        gz.extend_from_slice(&FINAL);
        gz.extend_from_slice(&crc.trailer());
        let mut out = vec![];
        flate2::read::GzDecoder::new(&gz[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, parts.concat());
        assert_eq!(crc.crc, crc32fast::hash(&parts.concat()));
    }
}
