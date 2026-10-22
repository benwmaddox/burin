root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

# reads become paths: (the table a query starts from, the dotted path from it); "*": its rows
edit('sql.rs', [
    ('''// ---- read sets: every (entity, column) the lowering turns into SQL while it is recording (a
// register's loader, a screen list's query). "*": the entity's rows themselves (a row made or
// deleted can enter or leave). Recorded where paths become columns, so nothing read is missed.''',
     '''// ---- read sets: every path the lowering turns into SQL while it is recording (a register's
// loader, a screen list's query): (the table it starts from, the dotted path from it), so the same
// column read along two paths (a message's creator's name, a boost's booster's name) is two reads.
// "*": the table's rows themselves (a row made or deleted can enter or leave). Recorded where
// paths become columns, so nothing read is missed.'''),
    ('''fn note(ent: &str, col: &str) {''', '''fn note_path(ent: &str, path: &[String]) {
    note(ent, &path.join("."));
}

fn note(ent: &str, col: &str) {'''),
    ('''        let (mut alias, mut ent) = (base.to_string(), base_ent.to_string());
        for i in 0..path.len() - 1 {
            let f = &path[i];
            note(&ent, f);''', '''        note_path(base_ent, path);
        let (mut alias, mut ent) = (base.to_string(), base_ent.to_string());
        for i in 0..path.len() - 1 {
            let f = &path[i];'''),
    ('''        let last = &path[path.len() - 1];
        note(&ent, last);
        if m.field_type(&ent, last).is_none() {''', '''        let last = &path[path.len() - 1];
        if m.field_type(&ent, last).is_none() {'''),
    ('''        1 => {
            note(ent, &p[0]);
            Ok(format!("{alias}.{}", q(&p[0])))
        }
        _ => {
            note(ent, &p[0]);
            let target = ref_target(m, ent, &p[0])?;''', '''        1 => {
            note_path(ent, p);
            Ok(format!("{alias}.{}", q(&p[0])))
        }
        _ => {
            note_path(ent, p);
            let target = ref_target(m, ent, &p[0])?;'''),
    ('''            let target = ref_target(m, ent, &p[0])?;
            let inner = sub_column(m, "x", &target, &p[1..])?;''', '''            let target = ref_target(m, ent, &p[0])?;
            // (the inner part is the same path, already noted from its start)
            let inner = READS.with(|r| {
                let saved = r.borrow_mut().take();
                let x = sub_column(m, "x", &target, &p[1..]);
                *r.borrow_mut() = saved;
                x
            })?;'''),
])
# the path's target: (entity, column) where a path from a table ends
edit('sql.rs', [
    ('''/// Runs `f`, returning what it read.''', '''/// Where a read path ends: (entity, column), "*" for a table's rows.
pub fn target(m: &Model, start: &str, path: &str) -> Option<(String, String)> {
    if path == "*" {
        return Some((start.to_string(), "*".into()));
    }
    let parts: Vec<&str> = path.split('.').collect();
    let mut ent = start.to_string();
    for f in &parts[..parts.len() - 1] {
        ent = ref_target(m, &ent, f).ok()?;
    }
    Some((ent, parts[parts.len() - 1].to_string()))
}

/// Runs `f`, returning what it read.'''),
])

edit('actions.rs', [
    # coverage by path: ('r'|'l', index, Some(start, path)) or None for the whole register/list
    ('''    /// the registers ('r') and screen lists ('l') a write's precise edges reached
    touched: Vec<(char, usize)>,''', '''    /// the registers ('r') and screen lists ('l') a write's precise edges reached: in full (None),
    /// or for one read path (its start table, its path)
    touched: Vec<(char, usize, Option<(String, String)>)>,'''),
    ('''        match op {
            1 | 3 | 4 | 6 | 9 => self.touched.push(('r', a)),
            2 | 5 | 7 => self.touched.push(('l', a)),
            _ => {}
        }''', '''        // ops that rebuild a register or list in full cover all its reads (op 3 covers the read
        // path its caller names: `cover`)
        match op {
            1 | 4 | 6 | 9 => self.touched.push(('r', a, None)),
            2 | 5 | 7 => self.touched.push(('l', a, None)),
            _ => {}
        }'''),
    ('''    fn outcome(&mut self, o: &str) -> usize {''', '''    /// A precise edge covers this read path of register `reg`.
    fn cover(&mut self, reg: usize, start: &str, path: &str) {
        self.touched.push(('r', reg, Some((start.to_string(), path.to_string()))));
    }

    fn outcome(&mut self, o: &str) -> usize {'''),
    # rooted set: covers (root, col); and a field through a reference of the row: every kept one
    ('''            } else if r.schema.fields.iter().any(|(p, _)| {
                let ps: Vec<String> = p.split('.').skip(1).map(String::from).collect();
                ps.len() == 1 && cols.contains(&ps[0])
            }) {
                let id = vals.get("id").cloned().unwrap_or_default();
                self.delta(3, ri, &id, "0");
                self.note(format!("set {ent}.{} → {}: READ (rebuilt)", cols.join(","), r.name));
            }''', '''            } else if r.schema.fields.iter().any(|(p, _)| {
                let ps: Vec<String> = p.split('.').skip(1).map(String::from).collect();
                ps.len() == 1 && cols.contains(&ps[0])
            }) {
                let id = vals.get("id").cloned().unwrap_or_default();
                self.delta(3, ri, &id, "0");
                for c in cols {
                    self.cover(ri, ent, c);
                }
                self.note(format!("set {ent}.{} → {}: READ (rebuilt)", cols.join(","), r.name));
            }
        }
        // a row-keyed register's fields through a reference to the written row (a message's
        // creator's name): every kept one rebuilt
        for (ri, r) in regs.iter().enumerate() {
            let Some((_, rent)) = &r.root else { continue };
            if rows_change {
                continue;
            }
            for (p, _) in &r.schema.fields {
                let ps: Vec<&str> = p.split('.').skip(1).collect();
                if ps.len() == 2 && cols.iter().any(|c| c == ps[1]) && self.m.field_type(rent, ps[0]).map(|t| matches!(t.strip_opt(), Type::Ref(e) if e == ent)).unwrap_or(false) {
                    self.delta(4, ri, "0", "0");
                    self.note(format!("set {ent}.{} → {} (through {rent}.{}): READ (every kept one rebuilt)", cols.join(","), r.name, ps[0]));
                    break;
                }
            }'''),
    # list direct, patch: covers (l.entity, col) and its rows
    ('''                            self.line(&format!("call void @etch_delta_patch(ptr %d, i64 {}, i64 {k}, i64 {id}, i64 {slot}, i64 {fi}, i64 {v})", l.reg));
                            self.touched.push(('r', l.reg));''', '''                            self.line(&format!("call void @etch_delta_patch(ptr %d, i64 {}, i64 {k}, i64 {id}, i64 {slot}, i64 {fi}, i64 {v})", l.reg));
                            self.cover(l.reg, &l.entity, c);'''),
    ('''                    (Some(k), false) => {
                        self.delta(3, l.reg, &k, "0");''', '''                    (Some(k), false) => {
                        self.delta(3, l.reg, &k, "0");
                        self.cover(l.reg, &l.entity, "*");
                        for c in cols {
                            self.cover(l.reg, &l.entity, c);
                        }'''),
    ('''                self.line(&format!("call void @etch_delta_find(ptr %d, i64 {}, i64 {id}, i64 {slot}, i64 {pi})", l.reg));
                self.touched.push(('r', l.reg));''', '''                self.line(&format!("call void @etch_delta_find(ptr %d, i64 {}, i64 {id}, i64 {slot}, i64 {pi})", l.reg));
                for c in cols {
                    self.cover(l.reg, &l.entity, &format!("{}.{c}", l.items.fields[pi].0.split('.').nth(1).unwrap_or("")));
                }'''),
    ('''                self.delta(3, l.reg, &k, "0");
                let _ = writeln!(self.b, "  br label %{h}\\n{done}:\\n  call i32 @sqlite3_reset(ptr {st})");''', '''                self.delta(3, l.reg, &k, "0");
                for c in cols {
                    self.cover(l.reg, &l.entity, &format!("{p0}.{c}"));
                }
                let _ = writeln!(self.b, "  br label %{h}\\n{done}:\\n  call i32 @sqlite3_reset(ptr {st})");'''),
    # the fail-safe, by path
    ('''        let hit = |reads: &[(String, String)]| reads.iter().any(|(e, c)| e == ent && if rows_change { c == "*" } else { cols.contains(c) });
        for (ri, r) in regs.iter().enumerate() {
            if !self.touched.contains(&('r', ri)) && hit(&r.reads) {''', '''        let m = self.m;
        let reaches = |start: &str, path: &str| match sql::target(m, start, path) {
            Some((e, c)) => e == ent && if rows_change { c == "*" } else { cols.contains(&c) },
            None => false,
        };
        let hit = |reads: &[(String, String)]| reads.iter().any(|(s, p)| reaches(s, p));
        let uncovered = |touched: &[(char, usize, Option<(String, String)>)], kind: char, i: usize, reads: &[(String, String)]| {
            if touched.iter().any(|(k, x, c)| *k == kind && *x == i && c.is_none()) {
                return false;
            }
            reads.iter().any(|(s, p)| reaches(s, p) && !touched.iter().any(|(k, x, c)| *k == kind && *x == i && c.as_ref() == Some(&(s.clone(), p.clone()))))
        };
        for (ri, r) in regs.iter().enumerate() {
            if uncovered(&self.touched, 'r', ri, &r.reads) {'''),
    ('''            if !self.touched.contains(&('l', si)) && hit(&s.reads) {''', '''            if uncovered(&self.touched, 'l', si, &s.reads) {'''),
])
print('ok')
