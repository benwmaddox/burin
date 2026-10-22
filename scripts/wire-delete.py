import re
p = '/mnt/d/code/etch/crates/etchc/src/actions.rs'
s = open(p, encoding='utf-8').read()

old_fe_start = s.index('                Stmt::ForEach(var, q, inner, _) => {')
old_fe_end = s.index('                Stmt::Set(..) => return Err("set is lowered inside for each only (for now)".into()),\n')
end_line = len('                Stmt::Set(..) => return Err("set is lowered inside for each only (for now)".into()),\n')
new = r'''                // `set x.f = v` and `delete x` on one row: a for each over its id
                Stmt::Set(o, _, _, span) | Stmt::Delete(o, span) => {
                    let ent = match self.m.type_of(o, scope).map_err(|x| x.msg)? {
                        Type::Ref(t) => t,
                        _ => return Err("set and delete take a row".into()),
                    };
                    let row = "__row".to_string();
                    let filter = Expr::Bin(BinOp::Eq, Box::new(Expr::Dot("id".into(), *span)), Box::new(o.clone()), *span);
                    let q = Query { source: Source::Entity(ent), filter: Some(filter), order: vec![], limit: None, window_last: None, span: *span };
                    let inner = match s {
                        Stmt::Set(_, f, v, sp) => Stmt::Set(Expr::Name(row.clone(), *sp), f.clone(), v.clone(), *sp),
                        _ => Stmt::Delete(Expr::Name(row.clone(), *span), *span),
                    };
                    self.stmts(&[Stmt::ForEach(row, q, vec![inner], *span)], scope, regs, lists, screens)?;
                }
                Stmt::ForEach(var, q, inner, _) => {
                    let ent = match &q.source {
                        Source::Entity(e) => e.clone(),
                        _ => return Err("for each over a view isn't lowered yet".into()),
                    };
                    let mut sets = vec![];
                    let mut delete = false;
                    for s in inner {
                        match s {
                            Stmt::Set(Expr::Name(o, _), f, v, _) if o == var => {
                                sets.push((f.clone(), v.clone()));
                            }
                            Stmt::Delete(Expr::Name(o, _), _) if o == var && inner.len() == 1 => delete = true,
                            _ => return Err("inside for each: `set x.f = v`s, or one `delete x`".into()),
                        }
                    }
                    for (f, v) in &sets {
                        self.check_max(&ent, f, v)?;
                    }
                    let filter = q.filter.as_ref().map(|f| inline::expr(self.m, f, &inline::Env::new(), 0, 0));
                    // the changed rows' columns the delta edges key by: lists' and screens' keys
                    let mut ret: Vec<String> = vec!["id".into()];
                    let keys = lists.iter().filter(|l| l.entity == ent).filter_map(|l| l.keyed_by.clone()).chain(screens.iter().filter(|l| l.entity == ent).filter_map(|l| l.keyed_by.clone()));
                    for k in keys {
                        if !ret.contains(&k) {
                            ret.push(k);
                        }
                    }
                    let sel = if delete {
                        sql::delete(self.m, &ent, filter.as_ref().ok_or("delete needs a filter")?, &ret)?
                    } else {
                        sql::update(self.m, &ent, filter.as_ref(), &sets, &ret)?
                    };
                    let st = self.stmt(&sel)?;
                    let (h, body, done) = (format!("uh{}", self.n), format!("ub{}", self.n), format!("ud{}", self.n));
                    self.n += 1;
                    let (rc, ok) = (self.v(), self.v());
                    let _ = writeln!(self.b, "  br label %{h}\n{h}:\n  {rc} = call i32 @sqlite3_step(ptr {st})\n  {ok} = icmp eq i32 {rc}, 100\n  br i1 {ok}, label %{body}, label %{done}\n{body}:");
                    let mut vals = HashMap::new();
                    for (i, c) in ret.iter().enumerate() {
                        let x = self.v();
                        self.line(&format!("{x} = call i64 @etch_col_i64(ptr {st}, i32 {i})"));
                        vals.insert(c.clone(), x);
                    }
                    if delete {
                        self.edges(&ent, &["*".into()], &vals, &[], regs, lists, screens, None, true)?;
                    } else {
                        let cols: Vec<String> = sets.iter().map(|(c, _)| c.clone()).filter(|c| !c.ends_with("__seq")).collect();
                        let real_sets: Vec<(String, Expr)> = sets.iter().filter(|(c, _)| !c.ends_with("__seq")).cloned().collect();
                        self.edges(&ent, &cols, &vals, &real_sets, regs, lists, screens, None, false)?;
                    }
                    let _ = writeln!(self.b, "  br label %{h}\n{done}:\n  call void @etch_done(ptr {st}, i32 {rc})");
                }
'''
s = s[:old_fe_start] + new + s[old_fe_end + end_line:]

# max checks on create
s = s.replace('''                Stmt::Create(ent, fields, _) => {
                    let gid''', '''                Stmt::Create(ent, fields, _) => {
                    for (f, v) in fields {
                        self.check_max(ent, f, v)?;
                    }
                    let gid''')

# helpers: outcome codes and max checks
s = s.replace('''    fn bind(&mut self, st: &str, i: usize, e: &Expr) -> Result<(), String> {''', '''    fn outcome(&mut self, o: &str) -> usize {
        if !self.outcomes.iter().any(|x| x == o) {
            self.outcomes.push(o.to_string());
        }
        self.outcomes.iter().position(|x| x == o).unwrap()
    }

    /// Text written to a field with `max N`: at most N characters, else the outcome `invalid`
    /// (checked here, before SQLite: the same rule as the table's CHECK).
    fn check_max(&mut self, ent: &str, f: &str, e: &Expr) -> Result<(), String> {
        let max = self.m.entities.get(ent).and_then(|x| x.fields.iter().find(|x| x.name == f)).and_then(|x| if matches!(x.ty.strip_opt(), Type::Text) { x.max } else { None });
        let (Some(n), Expr::Name(a, _)) = (max, e) else { return Ok(()) };
        if !self.args.iter().any(|(x, _)| x == a) {
            return Ok(());
        }
        let (v, _) = self.value(e)?;
        let (c, ok) = (self.v(), self.v());
        self.line(&format!("{c} = call i64 @etch_str_chars(ptr {v})"));
        self.line(&format!("{ok} = icmp sle i64 {c}, {n}"));
        let code = self.outcome("invalid");
        let (pass, fail) = (format!("pass{}", self.n), format!("fail{}", self.n));
        self.n += 1;
        let _ = writeln!(self.b, "  br i1 {ok}, label %{pass}, label %{fail}\n{pass}:");
        self.fails.push((fail, code));
        Ok(())
    }

    fn bind(&mut self, st: &str, i: usize, e: &Expr) -> Result<(), String> {''')
s = s.replace('''                    let code = match &r.otherwise {
                        Else::Outcome(o) => {
                            if !self.outcomes.contains(o) {
                                self.outcomes.push(o.clone());
                            }
                            self.outcomes.iter().position(|x| x == o).unwrap()
                        }''', '''                    let code = match &r.otherwise {
                        Else::Outcome(o) => self.outcome(o),''')
open(p, 'w', encoding='utf-8').write(s)
print('ok')
