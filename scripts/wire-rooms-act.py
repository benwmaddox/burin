root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

edit('actions.rs', [
    # allocas go to the entry block (a loop's body would otherwise grow the stack)
    ('''    fn line(&mut self, s: &str) {
        let _ = writeln!(self.b, "  {s}");
    }''', '''    fn line(&mut self, s: &str) {
        for l in s.split('\\n') {
            if l.trim_start().contains(" = alloca ") {
                let _ = writeln!(self.allocas, "  {}", l.trim_start());
            } else {
                let _ = writeln!(self.b, "  {}", l.trim_start());
            }
        }
    }'''),
    ('''    /// the authenticates seen so far (their index is their hidden argument's)
    verifies: Vec<(usize, Expr, String)>,
}''', '''    /// the authenticates seen so far (their index is their hidden argument's)
    verifies: Vec<(usize, Expr, String)>,
    /// the entry block's allocas
    allocas: String,
}'''),
    ('''edges: vec![], audiences: auds.clone(), lets: HashMap::new(), verifies: vec![] };''', '''edges: vec![], audiences: auds.clone(), lets: HashMap::new(), verifies: vec![], allocas: String::new() };'''),
    ('''            let _ = writeln!(f, "  %now = call i64 @etch_w_now(ptr %w)");
            f.push_str(&a.b.replace("%Args,", &format!("%Args_{name},")));''', '''            let _ = writeln!(f, "  %now = call i64 @etch_w_now(ptr %w)");
            f.push_str(&a.allocas);
            f.push_str(&a.b.replace("%Args,", &format!("%Args_{name},")));'''),
    # let: a fresh slot per let (the same name in two branches)
    ('''                    let (p, v) = (format!("%let_{n}"), self.v());''', '''                    let (p, v) = (format!("%let_{n}_{}", self.n), self.v());'''),
    # create as: the new row by name
    ('''                Stmt::Create(ent, fields, _) => {''', '''                Stmt::Create(ent, fields, name, _) => {'''),
    ('''                    self.line(&format!("call void @etch_exec(ptr {st})"));
                    self.edges(ent, &["*".into()], &vals, &[], regs, lists, screens, Some((&id, &seq, &fixed)), false)?;''', '''                    self.line(&format!("call void @etch_exec(ptr {st})"));
                    self.edges(ent, &["*".into()], &vals, &[], regs, lists, screens, Some((&id, &seq, &fixed)), false)?;
                    if let Some(n) = name {
                        let p = format!("%let_{n}_{}", self.n);
                        self.n += 1;
                        self.line(&format!("{p} = alloca i64"));
                        self.line(&format!("store i64 {id}, ptr {p}"));
                        self.rows.push((n.clone(), ent.clone()));
                        self.lets.insert(n.clone(), (p, Expr::None(Span::default()), Type::Ref(ent.clone())));
                    }'''),
    ('''                    self.stmts(&[Stmt::Create(via_entity.clone(), fields, *span)], scope, regs, lists, screens)?;''', '''                    self.stmts(&[Stmt::Create(via_entity.clone(), fields, None, *span)], scope, regs, lists, screens)?;'''),
    # if and go
    ('''                Stmt::SignOut(span) => {''', '''                Stmt::If(c, a, b, _) => {
                    let cond = inline::expr(self.m, c, &inline::Env::new(), 0, 0);
                    let sel = sql::cond_select_with(self.m, &cond, &self.rows)?;
                    let st = self.stmt(&sel)?;
                    let (x, ok) = (self.v(), self.v());
                    self.line(&format!("call i32 @sqlite3_step(ptr {st})"));
                    self.line(&format!("{x} = call i64 @etch_col_i64(ptr {st}, i32 0)"));
                    self.line(&format!("call i32 @sqlite3_reset(ptr {st})"));
                    self.line(&format!("{ok} = icmp eq i64 {x}, 1"));
                    let (lt, lf, le) = (format!("ift{}", self.n), format!("iff{}", self.n), format!("ife{}", self.n));
                    self.n += 1;
                    let _ = writeln!(self.b, "  br i1 {ok}, label %{lt}, label %{lf}\\n{lt}:");
                    // names made in a branch stay in it
                    let (lets, rows) = (self.lets.clone(), self.rows.clone());
                    self.stmts(a, scope, regs, lists, screens)?;
                    (self.lets, self.rows) = (lets.clone(), rows.clone());
                    let _ = writeln!(self.b, "  br label %{le}\\n{lf}:");
                    self.stmts(b, scope, regs, lists, screens)?;
                    (self.lets, self.rows) = (lets, rows);
                    let _ = writeln!(self.b, "  br label %{le}\\n{le}:");
                }
                Stmt::Go(screen, arg, _) => {
                    let (route, param) = self.m.screens.iter().find_map(|d| match d {
                        Decl::Screen { name, route, params, .. } if name == screen => Some((route.clone(), params[0].name.clone())),
                        _ => None,
                    }).ok_or("go's screen")?;
                    let (pre, post) = route.split_once(&format!("{{{param}}}")).ok_or("go's route")?;
                    let (gp, gs) = (self.md.str_const_pub(pre.as_bytes()), self.md.str_const_pub(post.as_bytes()));
                    let (id, _) = self.value(arg)?;
                    self.line(&format!("call void @etch_w_go(ptr %w, ptr {gp}, i64 {}, i64 {id}, ptr {gs}, i64 {})", pre.len(), post.len()));
                }
                Stmt::SignOut(span) => {'''),
    # for each: set-based when it only sets or deletes; else row by row
    ('''                Stmt::ForEach(var, q, inner, _) => {
                    let ent = match &q.source {
                        Source::Entity(e) => e.clone(),
                        _ => return Err("for each over a view isn't lowered yet".into()),
                    };''', '''                Stmt::ForEach(var, q, inner, _) if !inner.iter().all(|s| matches!(s, Stmt::Set(Expr::Name(o, _), ..) if o == var)) && !(inner.len() == 1 && matches!(&inner[0], Stmt::Delete(Expr::Name(o, _), _) if o == var)) => {
                    // row by row: the rows' ids first (in the writer's arena), then the body for each
                    let ent = match &q.source {
                        Source::Entity(e) => e.clone(),
                        _ => return Err("for each over a view isn't lowered yet".into()),
                    };
                    let filter = q.filter.as_ref().map(|f| inline::expr(self.m, f, &inline::Env::new(), 0, 0));
                    let sel = sql::ids(self.m, &ent, filter.as_ref(), &self.rows)?;
                    let st = self.stmt(&sel)?;
                    let (ids, p, n) = (self.v(), self.v(), self.v());
                    self.line(&format!("{ids} = call %List @etch_w_ids(ptr %w, ptr {st})"));
                    self.line(&format!("{p} = extractvalue %List {ids}, 0"));
                    self.line(&format!("{n} = extractvalue %List {ids}, 1"));
                    let k = self.n;
                    self.n += 1;
                    let (slot, ix) = (format!("%let_{var}_{k}"), format!("%ix{k}"));
                    self.line(&format!("{slot} = alloca i64"));
                    self.line(&format!("{ix} = alloca i64"));
                    self.line(&format!("store i64 0, ptr {ix}"));
                    let (h, body, done) = (format!("fh{k}"), format!("fb{k}"), format!("fd{k}"));
                    let (i, more, at, id) = (self.v(), self.v(), self.v(), self.v());
                    let _ = writeln!(self.b, "  br label %{h}\\n{h}:\\n  {i} = load i64, ptr {ix}\\n  {more} = icmp slt i64 {i}, {n}\\n  br i1 {more}, label %{body}, label %{done}\\n{body}:");
                    self.line(&format!("{at} = getelementptr inbounds i64, ptr {p}, i64 {i}"));
                    self.line(&format!("{id} = load i64, ptr {at}"));
                    self.line(&format!("store i64 {id}, ptr {slot}"));
                    let (lets, rows) = (self.lets.clone(), self.rows.clone());
                    self.lets.insert(var.clone(), (slot, Expr::None(Span::default()), Type::Ref(ent.clone())));
                    self.rows.push((var.clone(), ent.clone()));
                    self.stmts(inner, &scope.with(var, Type::Ref(ent.clone())), regs, lists, screens)?;
                    (self.lets, self.rows) = (lets, rows);
                    let (i2, nx) = (self.v(), self.v());
                    let _ = writeln!(self.b, "  {i2} = load i64, ptr {ix}\\n  {nx} = add i64 {i2}, 1\\n  store i64 {nx}, ptr {ix}\\n  br label %{h}\\n{done}:");
                }
                Stmt::ForEach(var, q, inner, _) => {
                    let ent = match &q.source {
                        Source::Entity(e) => e.clone(),
                        _ => return Err("for each over a view isn't lowered yet".into()),
                    };'''),
    # a deleted row whose screen it is: the screen's kept instance goes (op 13)
    ('''        // the screens' lists of this entity
        for (si, s) in screens.iter().enumerate() {''', '''        if removed {
            for (si, d) in self.m.screens.iter().enumerate() {
                if let Decl::Screen { params, .. } = d {
                    if matches!(params.first().map(|p| p.ty.strip_opt()), Some(Type::Ref(e)) if e == ent) {
                        let id = vals.get("id").cloned().ok_or("the deleted row's id")?;
                        self.delta(13, si, &id, "0");
                        self.note(format!("delete {ent} → screen {si} at it: dropped (no read)"));
                    }
                }
            }
        }
        // the screens' lists of this entity
        for (si, s) in screens.iter().enumerate() {'''),
    # list arguments: JSON arrays of ids (text)
    ('''            let mut args: Vec<(String, Shape)> = params.iter().map(|p| (p.name.clone(), if matches!(p.ty.strip_opt(), Type::Text) { Shape::Str } else { Shape::I64 })).collect();''', '''            let mut args: Vec<(String, Shape)> = params.iter().map(|p| (p.name.clone(), if matches!(p.ty.strip_opt(), Type::Text | Type::List(_)) { Shape::Str } else { Shape::I64 })).collect();
            let list_args: Vec<String> = params.iter().filter(|p| matches!(p.ty.strip_opt(), Type::List(_))).map(|p| p.name.clone()).collect();'''),
    ('''            infos.push(ActInfo { name: name.clone(), args, outcomes, edges, public: *public, verifies });''', '''            infos.push(ActInfo { name: name.clone(), args, outcomes, edges, public: *public, verifies, list_args });'''),
    ('''    pub verifies: Vec<(usize, Expr, String)>,
}''', '''    pub verifies: Vec<(usize, Expr, String)>,
    /// arguments that are lists of rows: from repeated fields, as JSON arrays of ids
    pub list_args: Vec<String>,
}'''),
    ('''            Stmt::Create(e, _, _) => out.push(e.clone()),
            Stmt::ForEach(_, _, inner, _) => creates_of(inner, out),''', '''            Stmt::Create(e, _, _, _) => out.push(e.clone()),
            Stmt::ForEach(_, _, inner, _) => creates_of(inner, out),
            Stmt::If(_, a, b, _) => {
                creates_of(a, out);
                creates_of(b, out);
            }'''),
    ('''                        Stmt::ForEach(_, _, inner, _) => hidden(inner, out, n),''', '''                        Stmt::ForEach(_, _, inner, _) => hidden(inner, out, n),
                        Stmt::If(_, a, b, _) => {
                            hidden(a, out, n);
                            hidden(b, out, n);
                        }'''),
])

edit('sql.rs', [
    ('''/// `DELETE FROM E AS u0 WHERE … RETURNING …`''', '''/// `SELECT u0.id FROM E AS u0 WHERE …`: the rows a for each runs over (`rows`: parameters and
/// names that are rows, their fields read by id).
pub fn ids(m: &Model, ent: &str, filter: Option<&Expr>, rows: &[(String, String)]) -> Result<Select, String> {
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: rows.to_vec(), rows: vec![("u0".into(), ent.to_string())], sub: 0 };
    let w = match filter {
        Some(f) => format!(" WHERE {}", tx.expr(f)?),
        None => String::new(),
    };
    Ok(Select { sql: format!("SELECT u0.id FROM {} AS u0{w} ORDER BY u0.id", q(ent)), binds: tx.binds })
}

/// `DELETE FROM E AS u0 WHERE … RETURNING …`'''),
])

edit('llvm.rs', [
    ('''declare void @etch_w_sign_out(ptr)''', '''declare void @etch_w_sign_out(ptr)
declare %List @etch_w_ids(ptr, ptr)
declare void @etch_w_go(ptr, ptr, i64, i64, ptr, i64)'''),
])

# the glue: list arguments from repeated fields
edit('glue.rs', [
    ('''                crate::llvm::Shape::Str => {
                    let _ = writeln!(s, "                let {n}_s = etch_rt::app::field(form, {n:?}).unwrap_or(\\"\\");");
                    fields.push(format!("{n}: Str::of({n}_s)"));
                }''', '''                crate::llvm::Shape::Str if a.list_args.contains(n) => {
                    let _ = writeln!(s, "                let {n}_s = etch_rt::app::field_ids(form, {n:?});");
                    fields.push(format!("{n}: Str::of(&{n}_s)"));
                }
                crate::llvm::Shape::Str => {
                    let _ = writeln!(s, "                let {n}_s = etch_rt::app::field(form, {n:?}).unwrap_or(\\"\\");");
                    fields.push(format!("{n}: Str::of({n}_s)"));
                }'''),
])
print('ok')
