//! The snapshot every app keeps (the "registers" of docs/IR.md), the same for every app: pieces of
//! pages by register kind and key, the screens' lists and audiences, the sessions. The compiler's
//! generated glue knows which kinds and lists an app has (by index); this knows only how to keep them.
//!
//! The writer builds the next snapshot sharing everything a commit didn't touch (`next`), and
//! publishes it by swapping one pointer; readers never lock.

use crate::gz::Piece;
use std::collections::HashMap;
use std::sync::Arc;

/// A piece of a page, as it is and precompressed (built once, at write time).
pub struct Seg {
    pub raw: Vec<u8>,
    pub gz: Piece,
}

impl Seg {
    pub fn new(raw: Vec<u8>) -> Seg {
        let gz = Piece::new(&raw);
        Seg { raw, gz }
    }
}

/// A cached register's value: one piece, the same for everyone (a cached fragment never reads
/// the viewer).
pub struct Reg {
    pub seg: Seg,
}

impl Reg {
    pub fn one(raw: Vec<u8>) -> Reg {
        Reg { seg: Seg::new(raw) }
    }
}

/// A page's parts, in order: a cached piece, or a fragment rendered for the viewer now (its
/// kind, its key) that merges the cached pieces it calls.
pub enum Part<'a> {
    Seg(&'a Seg),
    Render(usize, i64),
}

/// A screen instance's sorted audience (who may see it), and whether its write-time guards hold.
pub struct Screen {
    pub audience: Vec<i64>,
    /// the response's headers after the length (its type, its cookies, Vary)
    pub headers: Vec<u8>,
}

#[derive(Clone, Default)]
pub struct Snap {
    pub version: u64,
    pub sessions: HashMap<Box<[u8]>, i64>,
    /// by register kind (the generated index), by key
    pub regs: Vec<HashMap<i64, Arc<Reg>>>,
    /// by screen list, by key: the items' keys, in order
    pub lists: Vec<HashMap<i64, Arc<Vec<i64>>>>,
    /// by screen, by key: the instance's audience and headers (absent: not precomputed)
    pub screens: Vec<HashMap<i64, Arc<Screen>>>,
}

impl Snap {
    pub fn new(regs: usize, lists: usize, screens: usize) -> Snap {
        Snap { version: 0, sessions: HashMap::new(), regs: vec![HashMap::new(); regs], lists: vec![HashMap::new(); lists], screens: vec![HashMap::new(); screens] }
    }
    /// The next snapshot: everything shared, the version moved on.
    pub fn next(&self) -> Snap {
        let mut s = self.clone();
        s.version += 1;
        s
    }
    #[inline]
    pub fn reg(&self, kind: usize, key: i64) -> Option<&Reg> {
        self.regs[kind].get(&key).map(|r| &**r)
    }
}
