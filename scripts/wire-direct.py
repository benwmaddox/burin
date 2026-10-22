p = '/mnt/d/code/etch/crates/etchc/src/llvm.rs'
s = open(p, encoding='utf-8').read()

# 1. on_viewer sees into queries, comprehensions and ifs
old = '''    fn on_viewer(e: &Expr) -> bool {
        match e {
            Expr::Name(n, _) => n == "viewer",
            Expr::Field(o, _, _) | Expr::Not(o, _) => Self::on_viewer(o),
            Expr::Bin(_, l, r, _) => Self::on_viewer(l) || Self::on_viewer(r),
            Expr::Call(_, a, _) => a.iter().any(Self::on_viewer),
            _ => false,
        }
    }'''
new = '''    fn on_viewer(e: &Expr) -> bool {
        match e {
            Expr::Name(n, _) => n == "viewer",
            Expr::Field(o, _, _) | Expr::Not(o, _) => Self::on_viewer(o),
            Expr::Bin(_, l, r, _) => Self::on_viewer(l) || Self::on_viewer(r),
            Expr::Call(_, a, _) => a.iter().any(Self::on_viewer),
            Expr::If(c, a, b, _) => Self::on_viewer(c) || Self::on_viewer(a) || Self::on_viewer(b),
            Expr::Comp(_, x, q, _) => Self::on_viewer(x) || Self::on_viewer(&Expr::Query(q.clone())),
            Expr::Query(q) => {
                q.filter.as_ref().map(Self::on_viewer).unwrap_or(false)
                    || matches!(&q.source, Source::Expr(x) if Self::on_viewer(x))
                    || q.order.iter().any(|(k, _)| Self::on_viewer(k))
            }
            _ => false,
        }
    }

    /// A shared register's text may not depend on the viewer (only its conditions may: they are
    /// its variant bits). The value parts of a hole: not conditions, nor filters split out below.
    fn viewer_value(e: &Expr) -> bool {
        match e {
            Expr::Name(n, _) => n == "viewer",
            Expr::Field(o, _, _) => Self::viewer_value(o),
            Expr::If(_, a, b, _) => Self::viewer_value(a) || Self::viewer_value(b),
            Expr::Bin(BinOp::Add, l, r, _) => Self::viewer_value(l) || Self::viewer_value(r),
            Expr::Call(f, a, _) if f == "count" => a.iter().any(|x| match x {
                Expr::Query(q) => Self::viewer_source(q),
                x => Self::on_viewer(x),
            }),
            Expr::Call(_, a, _) => a.iter().any(Self::viewer_value),
            Expr::Comp(_, x, q, _) => Self::viewer_value(x) || Self::viewer_source(q),
            Expr::Query(q) => Self::viewer_source(q),
            _ => false,
        }
    }

    /// A query that depends on the viewer other than through filter conjuncts checked per item.
    fn viewer_source(q: &Query) -> bool {
        matches!(&q.source, Source::Expr(x) if Self::on_viewer(x)) || q.order.iter().any(|(k, _)| Self::on_viewer(k)) || q.limit.as_ref().map(Self::on_viewer).unwrap_or(false)
    }

    /// A query's filter split: the conjuncts on the viewer (checked per item, at build time: each
    /// a variant bit), and the query without them (loaded with the record).
    fn split_viewer(q: &Query) -> (Query, Option<Expr>) {
        let mut conj = vec![];
        fn split(e: &Expr, out: &mut Vec<Expr>) {
            if let Expr::Bin(BinOp::And, l, r, _) = e {
                split(l, out);
                split(r, out);
            } else {
                out.push(e.clone());
            }
        }
        if let Some(f) = &q.filter {
            split(f, &mut conj);
        }
        let (on, off): (Vec<Expr>, Vec<Expr>) = conj.into_iter().partition(Self::on_viewer);
        let join = |v: Vec<Expr>| v.into_iter().reduce(|a, b| Expr::Bin(BinOp::And, Box::new(a), Box::new(b), q.span));
        (Query { filter: join(off), ..q.clone() }, join(on))
    }

    /// A viewer-dependent condition's value, shifted into the variant key (a shared register).
    fn bit(&mut self, cv: &str) {
        if !self.bits || self.per_viewer {
            return;
        }
        self.max_bits += self.mult;
        let (k, z, s, o) = (self.v(), self.v(), self.v(), self.v());
        self.line(&format!("{k} = load i64, ptr %key"));
        self.line(&format!("{z} = zext i1 {cv} to i64"));
        self.line(&format!("{s} = shl i64 {k}, 1"));
        self.line(&format!("{o} = or i64 {s}, {z}"));
        self.line(&format!("store i64 {o}, ptr %key"));
    }'''
assert old in s; s = s.replace(old, new)

old = '''                    let cv = self.cond(roots, c)?;
                    if self.bits && !self.per_viewer && Self::on_viewer(c) {
                        self.max_bits += self.mult;
                        let (k, z, s, o) = (self.v(), self.v(), self.v(), self.v());
                        self.line(&format!("{k} = load i64, ptr %key"));
                        self.line(&format!("{z} = zext i1 {cv} to i64"));
                        self.line(&format!("{s} = shl i64 {k}, 1"));
                        self.line(&format!("{o} = or i64 {s}, {z}"));
                        self.line(&format!("store i64 {o}, ptr %key"));
                    }'''
new = '''                    let cv = self.cond(roots, c)?;
                    if Self::on_viewer(c) {
                        self.bit(&cv);
                    }'''
assert old in s; s = s.replace(old, new)

# Expr::If in text: its condition a bit too
old = '''        if let Expr::If(c, a, b, _) = e {
            let cv = self.cond(roots, c)?;'''
new = '''        if let Expr::If(c, a, b, _) = e {
            let cv = self.cond(roots, c)?;
            if Self::on_viewer(c) {
                self.bit(&cv);
            }'''
assert old in s; s = s.replace(old, new)

# sentence: viewer conjuncts checked per item
old = '''                    let t = self.tmp();
                    let list = as_list(qy);
                    let (inner, l) = self.open_loop(roots, &list, "it")?;
                    self.text(&inner, x, &t, false)?;
                    self.pending.push(0);
                    self.flush(&t);
                    self.close_loop(l);'''
new = '''                    let t = self.tmp();
                    let (rest, vc) = Self::split_viewer(qy);
                    let list = as_list(&rest);
                    let (inner, l) = self.open_loop(roots, &list, "it")?;
                    let skip = match &vc {
                        Some(c) => {
                            let cv = self.cond(&inner, c)?;
                            self.bit(&cv);
                            let (yes, no) = (self.label("in"), self.label("out"));
                            self.line(&format!("br i1 {cv}, label %{yes}, label %{no}"));
                            self.block(&yes);
                            Some(no)
                        }
                        None => None,
                    };
                    self.text(&inner, x, &t, false)?;
                    self.pending.push(0);
                    self.flush(&t);
                    if let Some(no) = skip {
                        self.line(&format!("br label %{no}"));
                        self.block(&no);
                    }
                    self.close_loop(l);'''
assert old in s; s = s.replace(old, new)

# count: of a list with viewer conjuncts, counted per item
old = '''            Expr::Call(f, a, _) if f == "count" => {
                let (parent, at, li) = self.list_slot(roots, &a[0], "it")?;'''
new = '''            Expr::Call(f, a, _) if f == "count" && matches!(&a[0], Expr::Query(q) if Self::split_viewer(q).1.is_some()) => {
                let Expr::Query(q) = &a[0] else { unreachable!() };
                let (rest, vc) = Self::split_viewer(q);
                let n = format!("%cnt{}", self.n + 1);
                self.n += 1;
                self.line(&format!("{n} = alloca i64\\n  store i64 0, ptr {n}"));
                let (inner, l) = self.open_loop(roots, &as_list(&rest), "it")?;
                let cv = self.cond(&inner, vc.as_ref().unwrap())?;
                self.bit(&cv);
                let (c0, z, c1) = (self.v(), self.v(), self.v());
                self.line(&format!("{c0} = load i64, ptr {n}\\n  {z} = zext i1 {cv} to i64\\n  {c1} = add i64 {c0}, {z}\\n  store i64 {c1}, ptr {n}"));
                self.close_loop(l);
                let r = self.v();
                self.line(&format!("{r} = load i64, ptr {n}"));
                Ok((r, Shape::I64))
            }
            Expr::Call(f, a, _) if f == "count" => {
                let (parent, at, li) = self.list_slot(roots, &a[0], "it")?;'''
assert old in s; s = s.replace(old, new)

# the refusal: a shared register's hole may not print a viewer-dependent value
old = '''                    let t = self.m.type_of(e, &self.scope).map_err(|x| format!("{}: {}", show(e), x.msg))?;
                    self.text(roots, e, out, t.strip_opt() != &Type::Html)?;'''
new = '''                    if !self.per_viewer && Self::viewer_value(e) {
                        return Err(format!(
                            "fragment {}: `{}` writes text that depends on the viewer; a shared fragment's text may depend on the viewer only through conditions (mark the fragment `per viewer`, or show it under an if)",
                            self.frag,
                            show(e)
                        ));
                    }
                    let t = self.m.type_of(e, &self.scope).map_err(|x| format!("{}: {}", show(e), x.msg))?;
                    self.text(roots, e, out, t.strip_opt() != &Type::Html)?;'''
assert old in s; s = s.replace(old, new)

# the key starts at 1: a mark before the bits, so keys from different paths (of different
# lengths) never collide; at most 63 bits then
old = '''            if bits {
                g.line("%key = alloca i64");
                g.line("store i64 0, ptr %key");
            }'''
new = '''            if bits {
                // the key starts with a 1: bit strings of different lengths stay different keys
                g.line("%key = alloca i64");
                g.line("store i64 1, ptr %key");
            }'''
assert old in s; s = s.replace(old, new)
old = '''    if out.3 > 64 {
        return Err(format!("fragment {name}: its variant key needs up to {} bits; at most 64 (bound its lists, or mark it `per viewer`)", out.3));
    }'''
new = '''    if out.3 > 63 {
        return Err(format!("fragment {name}: its variant key needs up to {} bits; at most 63 (bound its lists, or mark it `per viewer`)", out.3));
    }'''
assert old in s; s = s.replace(old, new)
open(p, 'w', encoding='utf-8').write(s)
print('ok')
