p = '/mnt/d/code/etch/crates/etchc/src/actions.rs'
s = open(p, encoding='utf-8').read()

# 1. the audiences: what each screen's viewer guard reads
old = '''struct Act<'a, 'b> {'''
new = '''/// A screen's audience (its guard `exists(E where .F == viewer and …)`): the entity it reads,
/// the column equal to the screen's parameter (its key), and every column its filter reads.
#[derive(Clone)]
pub struct Audience {
    pub screen: usize,
    pub entity: String,
    pub key_col: Option<String>,
    pub cols: Vec<String>,
}

/// Every screen's audience, by the screen's index.
pub fn audiences(m: &Model) -> Vec<Audience> {
    let mut out = vec![];
    for (si, d) in m.screens.iter().enumerate() {
        let Decl::Screen { params, requires, .. } = d else { continue };
        for r in requires {
            let cond = inline::expr(m, &r.cond, &inline::Env::new(), 0, 0);
            let Expr::Call(f, a, _) = &cond else { continue };
            let Some(Expr::Query(q)) = a.first() else { continue };
            let (Source::Entity(ent), Some(filter)) = (&q.source, &q.filter) else { continue };
            if f != "exists" || !mentions_viewer(filter) {
                continue;
            }
            let mut eqs = vec![];
            eq_conjuncts(filter, &mut eqs);
            let key_col = eqs.iter().find(|(_, x)| matches!(x, Expr::Name(n, _) if params.iter().any(|p| &p.name == n))).map(|(c, _)| c.clone());
            let mut paths = vec![];
            row_paths(filter, &mut paths);
            let mut cols: Vec<String> = paths.into_iter().filter_map(|p| p.first().cloned()).collect();
            cols.sort();
            cols.dedup();
            out.push(Audience { screen: si, entity: ent.clone(), key_col, cols });
        }
    }
    out
}

fn mentions_viewer(e: &Expr) -> bool {
    match e {
        Expr::Name(n, _) => n == "viewer",
        Expr::Field(o, _, _) | Expr::Not(o, _) => mentions_viewer(o),
        Expr::Bin(_, l, r, _) => mentions_viewer(l) || mentions_viewer(r),
        Expr::Call(_, a, _) => a.iter().any(mentions_viewer),
        _ => false,
    }
}

struct Act<'a, 'b> {'''
assert old in s; s = s.replace(old, new, 1)

old = '''    edges: Vec<String>,
}

impl<'a, 'b> Act<'a, 'b> {'''
new = '''    edges: Vec<String>,
    audiences: Vec<Audience>,
}

impl<'a, 'b> Act<'a, 'b> {'''
assert old in s; s = s.replace(old, new, 1)

# 2. the edge: a write to what an audience reads re-reads that audience (op 12)
old = '''        // the screens' lists of this entity
        for (si, s) in screens.iter().enumerate() {'''
new = '''        // the screens' audiences that read this entity: read again at the key (every key when
        // the key column itself changes: op 12 with no key)
        for a in self.audiences.clone() {
            if a.entity != ent || !(rows_change || cols.iter().any(|c| a.cols.contains(c))) {
                continue;
            }
            let moved = !rows_change && a.key_col.as_ref().map(|k| cols.contains(k)).unwrap_or(true);
            let key = if moved { None } else { a.key_col.as_ref().and_then(|k| vals.get(k).cloned()) };
            match key {
                Some(k) => {
                    self.delta(12, a.screen, &k, "0");
                    self.note(format!("{verb} {ent} → the audience of screen {}: READ (the audience at its key; the page rebuilt if it changed)", a.screen));
                }
                None => {
                    self.delta(12, a.screen, "-9223372036854775808", "0");
                    self.note(format!("{verb} {ent} → the audience of screen {}: READ (every kept instance's audience)", a.screen));
                }
            }
        }
        // the screens' lists of this entity
        for (si, s) in screens.iter().enumerate() {'''
assert old in s; s = s.replace(old, new, 1)

# 3. the deleted / updated rows return the audience's key column
old = '''                    let keys = lists.iter().filter(|l| l.entity == ent).filter_map(|l| l.keyed_by.clone()).chain(screens.iter().filter(|l| l.entity == ent).filter_map(|l| l.keyed_by.clone()));'''
new = '''                    let audience_keys: Vec<String> = self.audiences.iter().filter(|a| a.entity == ent).filter_map(|a| a.key_col.clone()).collect();
                    let keys = lists.iter().filter(|l| l.entity == ent).filter_map(|l| l.keyed_by.clone()).chain(screens.iter().filter(|l| l.entity == ent).filter_map(|l| l.keyed_by.clone())).chain(audience_keys);'''
assert old in s; s = s.replace(old, new, 1)

old = '''patches: vec![], finds: vec![], edges: vec![] };'''
new = '''patches: vec![], finds: vec![], edges: vec![], audiences: auds.clone() };'''
assert old in s; s = s.replace(old, new, 1)
old = '''    let mut all_finds: Vec<Patch> = vec![];
    for d in &m.actions {'''
new = '''    let mut all_finds: Vec<Patch> = vec![];
    let auds = audiences(m);
    for d in &m.actions {'''
assert old in s; s = s.replace(old, new, 1)
open(p, 'w', encoding='utf-8').write(s)

# the runtime: op 12
p = '/mnt/d/code/etch/crates/etch-rt/src/app.rs'
s = open(p, encoding='utf-8').read()
old = '''                // a deleted row's register
                9 => {
                    dropped.insert((a, key));
                }'''
new = '''                // a deleted row's register
                9 => {
                    dropped.insert((a, key));
                }
                // what a screen's audience reads changed: its audience again (every key: i64::MIN)
                12 => {
                    audiences.insert((a, key));
                }'''
assert old in s; s = s.replace(old, new, 1)
old = '''        let (mut shared, mut relist, mut dropped): (HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>) = Default::default();'''
new = '''        let (mut shared, mut relist, mut dropped, mut audiences): (HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>) = Default::default();'''
assert old in s; s = s.replace(old, new, 1)
old = '''        let mut found: HashMap<(usize, i64, i64), bool> = HashMap::new();'''
new = '''        // audiences that may have changed: read again; a kept instance whose audience changed is
        // rebuilt (its variants are indexed by place in the audience); a viewer who left can't be
        // served it, one who joined can
        for (s, key) in audiences {
            let keys: Vec<i64> = if key == i64::MIN { next.screens[s].keys().copied().collect() } else { vec![key] };
            for key in keys {
                let Some(inst) = next.screens[s].get(&key).cloned() else { continue };
                match G::screen(&c, s, key) {
                    Some((aud, _)) if aud == inst.audience => {}
                    _ => {
                        stats.audiences.fetch_add(1, Ordering::Relaxed);
                        build_screen::<G>(&c, &ctx, &mut next, &mut viewers, s, key);
                    }
                }
            }
        }
        let mut found: HashMap<(usize, i64, i64), bool> = HashMap::new();'''
assert old in s; s = s.replace(old, new, 1)
old = '''    pub cleared: AtomicU64,'''
new = '''    pub cleared: AtomicU64,
    /// screen instances rebuilt because their audience changed
    pub audiences: AtomicU64,'''
assert old in s; s = s.replace(old, new, 1)
s = s.replace('''"patched":{},"cleared":{},"version":{}}}"#,''', '''"patched":{},"cleared":{},"audiences":{},"version":{}}}"#,''', 1)
s = s.replace('''g(&st.patched), g(&st.cleared), self.snap''', '''g(&st.patched), g(&st.cleared), g(&st.audiences), self.snap''', 1)
open(p, 'w', encoding='utf-8').write(s)
print('ok')
