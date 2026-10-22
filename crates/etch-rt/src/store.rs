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

/// A register's value: its variants (one per kind of viewer, for a register with variant bits)
/// and which variant each audience member sees; or its pieces between splices (per viewer).
pub struct Reg {
    pub variants: Vec<Seg>,
    /// per audience place, the variant's index (empty: every viewer sees variant 0)
    pub by_place: Vec<u8>,
    /// more kinds of viewer than 256 variants: not kept (its pages are served cold)
    pub overflow: bool,
}

impl Reg {
    pub fn one(raw: Vec<u8>) -> Reg {
        Reg { variants: vec![Seg::new(raw)], by_place: vec![], overflow: false }
    }
    #[inline]
    pub fn for_place(&self, place: usize) -> &Seg {
        match self.by_place.get(place) {
            Some(&i) => &self.variants[i as usize],
            None => &self.variants[0],
        }
    }
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
        self.regs[kind].get(&key).map(|r| &**r).filter(|r| !r.overflow)
    }
}
