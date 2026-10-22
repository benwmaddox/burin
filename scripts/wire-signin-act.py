root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

edit('actions.rs', [
    ('''pub struct ActInfo {
    pub name: String,
    pub args: Vec<(String, Shape)>,
    pub outcomes: Vec<String>,
    /// how each delta edge is met: forwarded, patched, or read (for the report)
    pub edges: Vec<String>,
}''', '''pub struct ActInfo {
    pub name: String,
    pub args: Vec<(String, Shape)>,
    pub outcomes: Vec<String>,
    /// how each delta edge is met: forwarded, patched, or read (for the report)
    pub edges: Vec<String>,
    /// run without a viewer
    pub public: bool,
    /// each `authenticates(hash, text)`: (its index, the hash's expression in the action's
    /// arguments alone, the text's argument). The runtime reads the hash and verifies the text on
    /// the request's thread, then hands the writer the hash it verified (`__verified_<i>`): the
    /// writer checks only that the stored hash is still that one
    pub verifies: Vec<(usize, Expr, String)>,
}

/// `authenticates(h, p)` replaced by `h == __verified_<i>`, in order; the replaced (h, p) pairs.
fn verified(e: &Expr, out: &mut Vec<(Expr, Expr)>) -> Expr {
    match e {
        Expr::Call(f, a, s) if f == "authenticates" => {
            out.push((a[0].clone(), a[1].clone()));
            Expr::Bin(BinOp::Eq, Box::new(a[0].clone()), Box::new(Expr::Name(format!("__verified_{}", out.len() - 1), *s)), *s)
        }
        Expr::Bin(op, l, r, s) => Expr::Bin(op.clone(), Box::new(verified(l, out)), Box::new(verified(r, out)), *s),
        Expr::Not(x, s) => Expr::Not(Box::new(verified(x, out)), *s),
        other => other.clone(),
    }
}

/// Names replaced by expressions (an action's lets, by what they read).
fn subst(e: &Expr, env: &HashMap<String, Expr>) -> Expr {
    match e {
        Expr::Name(n, _) => env.get(n).cloned().unwrap_or_else(|| e.clone()),
        Expr::Field(o, f, s) => Expr::Field(Box::new(subst(o, env)), f.clone(), *s),
        Expr::Bin(op, l, r, s) => Expr::Bin(op.clone(), Box::new(subst(l, env)), Box::new(subst(r, env)), *s),
        Expr::Not(x, s) => Expr::Not(Box::new(subst(x, env)), *s),
        Expr::Call(f, a, s) => Expr::Call(f.clone(), a.iter().map(|x| subst(x, env)).collect(), *s),
        other => other.clone(),
    }
}'''),
    ('''    edges: Vec<String>,
    audiences: Vec<Audience>,
}''', '''    edges: Vec<String>,
    audiences: Vec<Audience>,
    /// lets: by name, the pointer their value is kept at, their expression (for the runtime's
    /// verification) and type
    lets: HashMap<String, (String, Expr, Type)>,
    /// the authenticates seen so far (their index is their hidden argument's)
    verifies: Vec<(usize, Expr, String)>,
}'''),
    ('''        match e {
            Expr::Name(n, _) if n == "viewer" => Ok(("%viewer".into(), Shape::I64)),''', '''        if let Expr::Name(n, _) = e {
            if let Some((p, _, _)) = self.lets.get(n) {
                let x = self.v();
                self.line(&format!("{x} = load i64, ptr {p}"));
                return Ok((x, Shape::I64));
            }
        }
        match e {
            Expr::Name(n, _) if n == "viewer" => Ok(("%viewer".into(), Shape::I64)),'''),
    # the require: authenticates replaced
    ('''                    let cond = inline::expr(self.m, &r.cond, &inline::Env::new(), 0, 0);
                    let sel = sql::cond_select_with(self.m, &cond, &self.rows)?;''', '''                    let cond = inline::expr(self.m, &r.cond, &inline::Env::new(), 0, 0);
                    let mut pairs = vec![];
                    let base = self.verifies.len();
                    let cond = {
                        // indexes continue from earlier requires
                        let mut all: Vec<(Expr, Expr)> = (0..base).map(|_| (Expr::None(Span::default()), Expr::None(Span::default()))).collect();
                        let c = verified(&cond, &mut all);
                        pairs = all.split_off(base);
                        c
                    };
                    let env: HashMap<String, Expr> = self.lets.iter().map(|(n, (_, e, _))| (n.clone(), e.clone())).collect();
                    for (i, (h, p)) in pairs.into_iter().enumerate() {
                        let arg = match p {
                            Expr::Name(a, _) if self.args.iter().any(|(x, s)| *x == a && *s == Shape::Str) => a,
                            _ => return Err("authenticates checks an argument's text".into()),
                        };
                        self.verifies.push((base + i, subst(&h, &env), arg));
                    }
                    let sel = sql::cond_select_with(self.m, &cond, &self.rows)?;'''),
    # let, sign in, sign out
    ('''                // `set x.f = v` and `delete x` on one row: a for each over its id''', '''                Stmt::Let(n, e, _) => {
                    let mut sc = scope.clone();
                    for (k, (_, _, t)) in &self.lets {
                        sc = sc.with(k, t.clone());
                    }
                    let t = self.m.type_of(e, &sc).map_err(|x| x.msg)?;
                    let x = inline::expr(self.m, e, &inline::Env::new(), 0, 0);
                    let sel = sql::cond_select_with(self.m, &x, &self.rows)?;
                    let st = self.stmt(&sel)?;
                    let (p, v) = (format!("%let_{n}"), self.v());
                    self.line(&format!("{p} = alloca i64"));
                    self.line(&format!("call i32 @sqlite3_step(ptr {st})"));
                    self.line(&format!("{v} = call i64 @etch_col_i64(ptr {st}, i32 0)"));
                    self.line(&format!("call i32 @sqlite3_reset(ptr {st})"));
                    self.line(&format!("store i64 {v}, ptr {p}"));
                    if let Type::Ref(ent) = t.strip_opt() {
                        self.rows.push((n.clone(), ent.clone()));
                    }
                    self.lets.insert(n.clone(), (p, x, t));
                }
                Stmt::SignIn(e, span) => {
                    let Some(Decl::Viewer { via_entity, via_field, to_field, .. }) = self.m.viewer else { return Err("sign in needs a viewer".into()) };
                    let (tok, _) = self.value(&Expr::Call("token".into(), vec![Expr::Int(32, *span)], *span))?;
                    let fields = vec![(to_field.clone(), e.clone()), (via_field.clone(), Expr::Name(format!("@ptr{tok}"), *span))];
                    self.stmts(&[Stmt::Create(via_entity.clone(), fields, *span)], scope, regs, lists, screens)?;
                    self.line(&format!("call void @etch_w_sign_in(ptr %w, ptr {tok})"));
                }
                Stmt::SignOut(span) => {
                    let Some(Decl::Viewer { via_entity, via_field, .. }) = self.m.viewer else { return Err("sign out needs a viewer".into()) };
                    let filter = Expr::Bin(BinOp::Eq, Box::new(Expr::Dot(via_field.clone(), *span)), Box::new(Expr::Name("__session".into(), *span)), *span);
                    let q = Query { source: Source::Entity(via_entity.clone()), filter: Some(filter), order: vec![], limit: None, window_last: None, span: *span };
                    let row = "__s".to_string();
                    self.stmts(&[Stmt::ForEach(row.clone(), q, vec![Stmt::Delete(Expr::Name(row, *span), *span)], *span)], scope, regs, lists, screens)?;
                    self.line("call void @etch_w_sign_out(ptr %w)");
                }
                // `set x.f = v` and `delete x` on one row: a for each over its id'''),
    # sessions in the snapshot: the viewer declaration's entity, by row id
    ('''        // the screens' audiences that read this entity''', '''        // the sessions the snapshot knows: a session row made, changed or deleted (op 10: read it
        // by its id; op 11: forget its token)
        if let Some(Decl::Viewer { via_entity, via_field, to_field, .. }) = self.m.viewer {
            if via_entity == ent && (rows_change || cols.iter().any(|c| c == via_field || c == to_field)) {
                let id = vals.get("id").cloned().ok_or("the session row's id")?;
                self.delta(if removed { 11 } else { 10 }, 0, &id, "0");
                self.note(format!("{verb} {ent} → the sessions: {}", if removed { "forgotten (no read)" } else { "READ (the row, by its id)" }));
            }
        }
        // the screens' audiences that read this entity'''),
    # lower: hidden arguments, public, verifies
    ('''        if let Decl::Action { name, params, body, .. } = d {
            let args: Vec<(String, Shape)> = params.iter().map(|p| (p.name.clone(), if matches!(p.ty.strip_opt(), Type::Text) { Shape::Str } else { Shape::I64 })).collect();''', '''        if let Decl::Action { name, params, body, public, .. } = d {
            let mut args: Vec<(String, Shape)> = params.iter().map(|p| (p.name.clone(), if matches!(p.ty.strip_opt(), Type::Text) { Shape::Str } else { Shape::I64 })).collect();
            // hidden arguments the runtime fills (never from the form): the request's session
            // token (sign out), each verified password hash
            fn hidden(body: &[Stmt], out: &mut Vec<String>, n: &mut usize) {
                for s in body {
                    match s {
                        Stmt::SignOut(_) if !out.iter().any(|x| x == "__session") => out.push("__session".into()),
                        Stmt::Require(r) => {
                            let mut v = vec![];
                            verified(&r.cond, &mut v);
                            for _ in v {
                                out.push(format!("__verified_{n}"));
                                *n += 1;
                            }
                        }
                        Stmt::ForEach(_, _, inner, _) => hidden(inner, out, n),
                        _ => {}
                    }
                }
            }
            let mut hid = vec![];
            hidden(body, &mut hid, &mut 0);
            args.extend(hid.into_iter().map(|h| (h, Shape::Str)));'''),
    ('''edges: vec![], audiences: auds.clone() };''', '''edges: vec![], audiences: auds.clone(), lets: HashMap::new(), verifies: vec![] };'''),
    ('''            let (outcomes, edges, patches) = (a.outcomes.clone(), a.edges.clone(), a.patches.clone());''', '''            let (outcomes, edges, patches, verifies) = (a.outcomes.clone(), a.edges.clone(), a.patches.clone(), a.verifies.clone());'''),
    ('''            infos.push(ActInfo { name: name.clone(), args, outcomes, edges });''', '''            infos.push(ActInfo { name: name.clone(), args, outcomes, edges, public: *public, verifies });'''),
])

edit('llvm.rs', [
    ('''declare i64 @etch_str_chars(ptr)''', '''declare i64 @etch_str_chars(ptr)
declare void @etch_w_sign_in(ptr, ptr)
declare void @etch_w_sign_out(ptr)'''),
])
print('ok')
