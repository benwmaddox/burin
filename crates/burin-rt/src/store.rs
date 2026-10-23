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
    /// absent for a list's item: sent inside its list's block, compressed as one
    pub gz: Option<Piece>,
}

impl Seg {
    pub fn new(raw: Vec<u8>) -> Seg {
        let gz = Some(Piece::new(&raw));
        Seg { raw, gz }
    }
    pub fn plain(raw: Vec<u8>) -> Seg {
        Seg { raw, gz: None }
    }
}

/// A list instance's items as one piece, compressed together (the window of a room's messages:
/// shared text compresses far better as one stream than item by item). It notes the list and the
/// items it was made from, so a stale block is never served: it's checked against the snapshot.
pub struct Block {
    pub list: Arc<Vec<i64>>,
    pub items: Vec<Arc<Reg>>,
    pub seg: Seg,
}

/// A cached register's value: one piece, the same for everyone (a cached fragment never reads
/// the viewer).
pub struct Reg {
    pub seg: Seg,
    /// a list item's element id (its first element's `id`): what a live change replaces or removes
    pub dom_id: Option<Box<str>>,
}

impl Reg {
    pub fn one(raw: Vec<u8>) -> Reg {
        Reg { seg: Seg::new(raw), dom_id: None }
    }
    /// A list's item: sent inside its list's block, and found on a page by its element's id.
    pub fn plain(raw: Vec<u8>) -> Reg {
        let dom_id = first_id(&raw);
        Reg { seg: Seg::plain(raw), dom_id }
    }
}

/// A page's parts, in order: a cached piece, or a fragment rendered for the viewer now (its
/// kind, its key) that merges the cached pieces it calls.
pub enum Part<'a> {
    Seg(&'a Seg),
    Render(usize, i64),
    /// where the page's live stream connects (`{{live()}}`): its screen, key and the snapshot's
    /// last event
    Live,
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
    /// the last live event this snapshot includes (a page from it carries it: its stream gets the
    /// ones after)
    pub live_seq: u64,
    pub sessions: HashMap<Box<[u8]>, i64>,
    /// by register kind (the generated index), by key
    pub regs: Vec<HashMap<i64, Arc<Reg>>>,
    /// by screen list, by key: the items' keys, in order
    pub lists: Vec<HashMap<i64, Arc<Vec<i64>>>>,
    /// by screen, by key: the instance's audience and headers (absent: not precomputed)
    pub screens: Vec<HashMap<i64, Arc<Screen>>>,
    /// by screen list, by key: the items as one compressed piece (absent or stale: item by item)
    pub blocks: Vec<HashMap<i64, Arc<Block>>>,
}

impl Snap {
    pub fn new(regs: usize, lists: usize, screens: usize) -> Snap {
        Snap { version: 0, live_seq: 0, sessions: HashMap::new(), regs: vec![HashMap::new(); regs], lists: vec![HashMap::new(); lists], screens: vec![HashMap::new(); screens], blocks: vec![HashMap::new(); lists] }
    }
    /// The next snapshot: everything shared, the version moved on.
    pub fn next(&self) -> Snap {
        let mut s = self.clone();
        s.version += 1;
        s
    }
    /// A list instance's block, when it was made from the items the snapshot has now.
    #[inline]
    pub fn block(&self, list: usize, key: i64, kind: usize) -> Option<&Seg> {
        let b = self.blocks[list].get(&key)?;
        let items = self.lists[list].get(&key)?;
        if !Arc::ptr_eq(&b.list, items) && b.list != *items {
            return None;
        }
        for (id, r) in items.iter().zip(&b.items) {
            if !self.regs[kind].get(id).is_some_and(|x| Arc::ptr_eq(x, r)) {
                return None;
            }
        }
        Some(&b.seg)
    }
    /// Blocks for the lists whose blocks are missing or stale (the writer, before it publishes):
    /// how many were made. `item` is a list's item kind.
    pub fn refresh_blocks(&mut self, item: impl Fn(usize) -> usize) -> usize {
        let mut made = 0;
        for l in 0..self.lists.len() {
            let kind = item(l);
            self.blocks[l].retain(|k, _| self.lists[l].contains_key(k));
            let stale: Vec<i64> = self.lists[l].keys().copied().filter(|k| self.block(l, *k, kind).is_none()).collect();
            for key in stale {
                let list = self.lists[l][&key].clone();
                let Some(items) = list.iter().map(|id| self.regs[kind].get(id).cloned()).collect::<Option<Vec<_>>>() else {
                    self.blocks[l].remove(&key);
                    continue;
                };
                let mut raw = Vec::with_capacity(items.iter().map(|r| r.seg.raw.len()).sum());
                for r in &items {
                    raw.extend_from_slice(&r.seg.raw);
                }
                let gz = Some(Piece::at(&raw, flate2::Compression::new(6)));
                self.blocks[l].insert(key, Arc::new(Block { list, items, seg: Seg { raw, gz } }));
                made += 1;
            }
        }
        made
    }
    #[inline]
    pub fn reg(&self, kind: usize, key: i64) -> Option<&Reg> {
        self.regs[kind].get(&key).map(|r| &**r)
    }
}

/// The `id` of a piece's first element (`<div id="message_7" …>`), if it has one.
pub fn first_id(raw: &[u8]) -> Option<Box<str>> {
    let tag_end = raw.iter().position(|&c| c == b'>')?;
    let tag = &raw[..tag_end];
    let at = tag.windows(4).position(|w| w == b"id=\"")?;
    if at > 0 && !tag[at - 1].is_ascii_whitespace() {
        return None;
    }
    let rest = &tag[at + 4..];
    let end = rest.iter().position(|&c| c == b'"')?;
    std::str::from_utf8(&rest[..end]).ok().map(Into::into)
}

#[cfg(test)]
mod id_tests {
    #[test]
    fn a_pieces_first_elements_id() {
        assert_eq!(super::first_id(b"<div id=\"message_7\" class=\"m\"><b id=\"x\">").as_deref(), Some("message_7"));
        assert_eq!(super::first_id(b"<div data-id=\"7\">").as_deref(), None);
        assert_eq!(super::first_id(b"<li class=\"a\" id=\"n-2\">").as_deref(), Some("n-2"));
    }
}
