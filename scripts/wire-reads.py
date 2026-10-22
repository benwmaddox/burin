root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

edit('sql.rs', [
    ('''fn q(name: &str) -> String {''', '''// ---- read sets: every (entity, column) the lowering turns into SQL while it is recording (a
// register's loader, a screen list's query). "*": the entity's rows themselves (a row made or
// deleted can enter or leave). Recorded where paths become columns, so nothing read is missed.

thread_local! {
    static READS: std::cell::RefCell<Option<Vec<(String, String)>>> = const { std::cell::RefCell::new(None) };
}

fn note(ent: &str, col: &str) {
    READS.with(|r| {
        if let Some(v) = r.borrow_mut().as_mut() {
            let x = (ent.to_string(), col.to_string());
            if !v.contains(&x) {
                v.push(x);
            }
        }
    });
}

/// Runs `f`, returning what it read.
pub fn reading<R>(f: impl FnOnce() -> R) -> (R, Vec<(String, String)>) {
    let outer = READS.with(|r| r.borrow_mut().replace(vec![]));
    let x = f();
    let got = READS.with(|r| std::mem::replace(&mut *r.borrow_mut(), outer)).unwrap_or_default();
    // a recording inside another: the outer one reads it too
    for (e, c) in &got {
        note(e, c);
    }
    (x, got)
}

fn q(name: &str) -> String {'''),
    ('''        let (mut alias, mut ent) = (base.to_string(), base_ent.to_string());
        for i in 0..path.len() - 1 {
            let f = &path[i];''', '''        let (mut alias, mut ent) = (base.to_string(), base_ent.to_string());
        for i in 0..path.len() - 1 {
            let f = &path[i];
            note(&ent, f);'''),
    ('''        let last = &path[path.len() - 1];
        if m.field_type(&ent, last).is_none() {''', '''        let last = &path[path.len() - 1];
        note(&ent, last);
        if m.field_type(&ent, last).is_none() {'''),
    ('''        1 => Ok(format!("{alias}.{}", q(&p[0]))),
        _ => {
            let target = ref_target(m, ent, &p[0])?;''', '''        1 => {
            note(ent, &p[0]);
            Ok(format!("{alias}.{}", q(&p[0])))
        }
        _ => {
            note(ent, &p[0]);
            let target = ref_target(m, ent, &p[0])?;'''),
    ('''        self.sub += 1;
        let a = format!("s{}", self.sub);
        self.rows.push((a.clone(), ent.clone()));''', '''        self.sub += 1;
        let a = format!("s{}", self.sub);
        note(&ent, "*");
        self.rows.push((a.clone(), ent.clone()));'''),
    ('''    let (qy, ent) = resolve(m, list, scope)?;
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: vec![], rows: vec![("t0".into(), ent.clone())], sub: 0 };
    let mut cols = vec![];''', '''    let (qy, ent) = resolve(m, list, scope)?;
    note(&ent, "*");
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: vec![], rows: vec![("t0".into(), ent.clone())], sub: 0 };
    let mut cols = vec![];'''),
])

edit('llvm.rs', [
    ('''    /// for a `per viewer` register: its splices' registers, in order
    pub splices: Vec<String>,
}''', '''    /// for a `per viewer` register: its splices' registers, in order
    pub splices: Vec<String>,
    /// every (entity, column) its loader reads ("*": the entity's rows)
    pub reads: Vec<(String, String)>,
}'''),
    ('''    md.registers.push(RegInfo { name: name.to_string(), schema, tmps: out.2, bits: out.3, per_viewer, root, splices: names });''',
     '''    md.registers.push(RegInfo { name: name.to_string(), schema, tmps: out.2, bits: out.3, per_viewer, root, splices: names, reads: vec![] });'''),
    ('''    let reg = RegInfo { name: "viewer".into(), schema: md.viewer.clone(), tmps: 0, bits: 0, per_viewer: false, root: Some(("viewer".into(), ent)), splices: vec![] };''',
     '''    let reg = RegInfo { name: "viewer".into(), schema: md.viewer.clone(), tmps: 0, bits: 0, per_viewer: false, root: Some(("viewer".into(), ent)), splices: vec![], reads: vec![] };'''),
    ('''    let regs = md.registers.clone();
    for reg in &regs {
        let f = loader(m, &mut md, reg, &mut qid)?;''', '''    let regs = md.registers.clone();
    let mut reads_of = vec![];
    for reg in &regs {
        let (f, reads) = crate::sql::reading(|| loader(m, &mut md, reg, &mut qid));
        let f = f?;
        reads_of.push(reads);'''),
    ('''    let vl = viewer_loader(m, &mut md, &mut qid)?;''', '''    for (r, reads) in md.registers.iter_mut().zip(reads_of) {
        r.reads = reads;
    }
    let (vl, viewer_reads) = crate::sql::reading(|| viewer_loader(m, &mut md, &mut qid));
    let vl = vl?;
    md.viewer_reads = viewer_reads;'''),
    ('''    pub funcs: String,
    pub registers: Vec<RegInfo>,
}''', '''    pub funcs: String,
    pub registers: Vec<RegInfo>,
    /// what the viewer's record reads (its variant bits' inputs)
    pub viewer_reads: Vec<(String, String)>,
}'''),
    ('''    let mut md = Module { globals: String::new(), strs: HashMap::new(), viewer: Schema::default(), funcs: String::new(), registers: vec![] };''',
     '''    let mut md = Module { globals: String::new(), strs: HashMap::new(), viewer: Schema::default(), funcs: String::new(), registers: vec![], viewer_reads: vec![] };'''),
])

edit('actions.rs', [
    ('''    /// newest first, with a limit: a new row goes first
    pub newest_first: bool,
}''', '''    /// newest first, with a limit: a new row goes first
    pub newest_first: bool,
    /// every (entity, column) its query reads
    pub reads: Vec<(String, String)>,
}'''),
    ('''                    out.push(ScreenList { name: show(list), expr: list.clone(), entity: ent, keyed_by, monotone, newest_first });''', '''                    let var = match n {
                        TNode::For(v, ..) => v.clone(),
                        _ => unreachable!(),
                    };
                    let (sel, reads) = sql::reading(|| sql::list_select(m, list, &[var], &Scope::of(params)));
                    sel?;
                    out.push(ScreenList { name: show(list), expr: list.clone(), entity: ent, keyed_by, monotone, newest_first, reads });'''),
    # what each edges call touched
    ('''    fn delta(&mut self, op: i32, a: usize, key: &str, item: &str) {''', '''    fn delta(&mut self, op: i32, a: usize, key: &str, item: &str) {
        match op {
            1 | 3 | 4 | 6 | 9 => self.touched.push(('r', a)),
            2 | 5 | 7 => self.touched.push(('l', a)),
            _ => {}
        }'''),
    ('''    /// the entry block's allocas
    allocas: String,
}''', '''    /// the entry block's allocas
    allocas: String,
    /// the registers ('r') and screen lists ('l') a write's precise edges reached
    touched: Vec<(char, usize)>,
}'''),
    ('''verifies: vec![], allocas: String::new() };''', '''verifies: vec![], allocas: String::new(), touched: vec![] };'''),
    ('''        let created = new.is_some();
        // rows enter or leave lists''', '''        let created = new.is_some();
        self.touched.clear();
        // rows enter or leave lists'''),
    ('''                            self.line(&format!("call void @etch_delta_patch(ptr %d, i64 {}, i64 {k}, i64 {id}, i64 {slot}, i64 {fi}, i64 {v})", l.reg));''', '''                            self.line(&format!("call void @etch_delta_patch(ptr %d, i64 {}, i64 {k}, i64 {id}, i64 {slot}, i64 {fi}, i64 {v})", l.reg));
                            self.touched.push(('r', l.reg));'''),
    ('''                self.line(&format!("call void @etch_delta_find(ptr %d, i64 {}, i64 {id}, i64 {slot}, i64 {pi})", l.reg));''', '''                self.line(&format!("call void @etch_delta_find(ptr %d, i64 {}, i64 {id}, i64 {slot}, i64 {pi})", l.reg));
                self.touched.push(('r', l.reg));'''),
    # the fail-safe, at the end of edges
    ('''                    self.delta(op, si, &k, id);
                    self.note(format!("create {ent} → screen list {}: {how}", s.name));
                }
            }
        }
        Ok(())
    }''', '''                    self.delta(op, si, &k, id);
                    self.note(format!("create {ent} → screen list {}: {how}", s.name));
                }
            }
        }
        // the fail-safe: anything that reads what was written (its read set, recorded where its
        // SQL was made) and that no precise edge reached is rebuilt in full. A missing rule costs
        // speed, never correctness; the build reports it
        let hit = |reads: &[(String, String)]| reads.iter().any(|(e, c)| e == ent && (rows_change || cols.contains(c)));
        for (ri, r) in regs.iter().enumerate() {
            if !self.touched.contains(&('r', ri)) && hit(&r.reads) {
                // a row-keyed register of a new row isn't kept yet; of a deleted one, dropped
                if rows_change && r.root.as_ref().map(|(_, e)| e == ent).unwrap_or(false) {
                    continue;
                }
                self.delta(4, ri, "0", "0");
                self.note(format!("{verb} {ent}.{} → {}: FALLBACK (every kept one rebuilt; no precise rule)", cols.join(","), r.name));
            }
        }
        for (si, s) in screens.iter().enumerate() {
            if !self.touched.contains(&('l', si)) && hit(&s.reads) {
                self.delta(5, si, "-9223372036854775808", "0");
                self.note(format!("{verb} {ent}.{} → screen list {}: FALLBACK (every kept one read again; no precise rule)", cols.join(","), s.name));
            }
        }
        if hit(&self.md.viewer_reads.clone()) {
            self.delta(15, 0, "0", "0");
            self.note(format!("{verb} {ent}.{} → the viewers' records: FALLBACK (all read again, the screens rebuilt)", cols.join(",")));
        }
        Ok(())
    }'''),
])
print('ok')
