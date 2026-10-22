# One-off: newest-first lists (prepend), and the schema an app creates.
def edit(p, old, new):
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == 1, (p, old[:80], s.count(old))
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)

A = 'D:/code/etch/crates/etchc/src/actions.rs'
G = 'D:/code/etch/crates/etchc/src/glue.rs'
R = 'D:/code/etch/crates/etch-rt/src/app.rs'

# a list ordered by creation: oldest first with a window (a new row appends), or newest first with
# a limit (a new row is first); either way the row past the bound drops
edit(A, '''    pub monotone: bool,
}''', '''    pub monotone: bool,
    /// newest first, with a limit: a new row goes first
    pub newest_first: bool,
}''')
edit(A, '''                    let monotone = matches!(q.order.first(), Some((Expr::Dot(f, _), false)) if f == "created") && q.window_last.is_some();
                    out.push(ScreenList { name: show(list), expr: list.clone(), entity: ent, keyed_by, monotone });''',
     '''                    let monotone = matches!(q.order.first(), Some((Expr::Dot(f, _), false)) if f == "created") && q.window_last.is_some();
                    let newest_first = matches!(q.order.first(), Some((Expr::Dot(f, _), true)) if f == "created") && q.limit.is_some() && q.window_last.is_none();
                    out.push(ScreenList { name: show(list), expr: list.clone(), entity: ent, keyed_by, monotone, newest_first });''')
edit(A, '''                    self.delta(if s.monotone { 2 } else { 5 }, si, &k, id);
                    self.note(format!("create {ent} → screen list {}: {}", s.name, if s.monotone { "appended (monotone order)" } else { "READ (rebuilt)" }));''',
     '''                    let (op, how) = if s.monotone {
                        (2, "appended (oldest first: the new row is last)")
                    } else if s.newest_first {
                        (7, "prepended (newest first: the new row is first)")
                    } else {
                        (5, "READ (rebuilt)")
                    };
                    self.delta(op, si, &k, id);
                    self.note(format!("create {ent} → screen list {}: {how}", s.name));''')

# the schema: a table per entity (id, seq, created, updated, its fields), an index per reference
edit(G, '''    let _ = writeln!(s, "    fn after_act(_name: &str, _form: &[(String, String)]) -> String {{ \\"/\\".to_string() }}");''',
     '''    let _ = writeln!(s, "    fn after_act(_name: &str, _form: &[(String, String)]) -> String {{ \\"/\\".to_string() }}");
    let _ = writeln!(s, "    fn schema() -> &'static str {{ r##\\"{}\\"## }}", schema(m));''')
edit(G, '''fn split_and(e: &Expr, out: &mut Vec<Expr>) {''', '''/// The tables an app's entities are kept in (created if missing; an existing table is left as it is).
pub fn schema(m: &Model) -> String {
    let mut s = String::new();
    let mut ents: Vec<&&Entity> = m.entities.values().collect();
    ents.sort_by(|a, b| a.name.cmp(&b.name));
    for e in ents {
        let mut cols = vec!["id INTEGER PRIMARY KEY".to_string(), "seq INTEGER NOT NULL".into(), "created INTEGER NOT NULL".into(), "updated INTEGER NOT NULL".into()];
        for f in &e.fields {
            let ty = match f.ty.strip_opt() {
                Type::Text => "TEXT",
                _ => "INTEGER",
            };
            let null = if matches!(f.ty, Type::Opt(_)) { "" } else { " NOT NULL" };
            let check = match (f.max, f.ty.strip_opt()) {
                (Some(n), Type::Text) => format!(" CHECK (length(\\"{}\\") <= {n})", f.name),
                _ => String::new(),
            };
            cols.push(format!("\\"{}\\" {ty}{null}{check}", f.name));
        }
        s.push_str(&format!("CREATE TABLE IF NOT EXISTS \\"{}\\" ({}) STRICT;\\n", e.name, cols.join(", ")));
        for f in &e.fields {
            if matches!(f.ty.strip_opt(), Type::Ref(_)) {
                s.push_str(&format!("CREATE INDEX IF NOT EXISTS \\"ix_{}_{}\\" ON \\"{}\\" (\\"{}\\");\\n", e.name, f.name, e.name, f.name));
            }
        }
        s.push_str(&format!("CREATE INDEX IF NOT EXISTS \\"ix_{}_created\\" ON \\"{}\\" (created, seq);\\n", e.name, e.name));
    }
    s
}

fn split_and(e: &Expr, out: &mut Vec<Expr>) {''')

# the runtime: the schema at start, and prepend
edit(R, '''    /// The page to go to after an action (its form fields).
    fn after_act(name: &str, form: &[(String, String)]) -> String;''', '''    /// The page to go to after an action (its form fields).
    fn after_act(name: &str, form: &[(String, String)]) -> String;
    /// The tables the app keeps its entities in (created if missing).
    fn schema() -> &'static str;''')
edit(R, '''        let w = open_write(db_path);
        let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);''', '''        let w = open_write(db_path);
        w.execute_batch(G::schema()).expect("the schema");
        let mut snap = Snap::new(G::REGS, G::LISTS, G::SCREENS);''')
edit(R, '''                6 => {
                    forwarded.insert(key, item);
                }
                2 => {''', '''                6 => {
                    forwarded.insert(key, item);
                }
                2 | 7 => {''')
edit(R, '''                    let mut items: Vec<i64> = next.lists[a].get(&key).map(|l| (**l).clone()).unwrap_or_default();
                    items.push(item);
                    // a window: the oldest beyond its bound (the spec's) drops
                    while items.len() > G::list_bound(a).max(1) {
                        let old = items.remove(0);
                        next.regs[kind].remove(&old);
                    }''', '''                    let mut items: Vec<i64> = next.lists[a].get(&key).map(|l| (**l).clone()).unwrap_or_default();
                    // oldest first (op 2): appended, the oldest drops; newest first (op 7): first, the last drops
                    if op == 2 {
                        items.push(item);
                        while items.len() > G::list_bound(a).max(1) {
                            let old = items.remove(0);
                            next.regs[kind].remove(&old);
                        }
                    } else {
                        items.insert(0, item);
                        while items.len() > G::list_bound(a).max(1) {
                            let old = items.pop().unwrap();
                            next.regs[kind].remove(&old);
                        }
                    }''')
print('ok')
