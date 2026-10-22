p = '/mnt/d/code/etch/crates/etchc/src/llvm.rs'
s = open(p, encoding='utf-8').read()

s = s.replace('''    splices: Vec<Expr>,
}''', '''    splices: Vec<Expr>,
    /// the variant key's per-item viewer conditions over the record's own lists, recorded once
    /// at its start (however often and wherever the template checks them): their keys
    hoisted: Vec<String>,
    prologue: String,
}''', 1)
s = s.replace('''                splices: vec![],
            };''', '''                splices: vec![],
                hoisted: vec![],
                prologue: String::new(),
            };''', 1)
old = '''            let (b, sch, tmps, maxb, sp) = (g.body, g.schema, g.tmps, g.max_bits, g.splices);'''
new = '''            let (b, sch, tmps, maxb, sp) = (if bits { g.prologue + &g.body } else { g.body }, g.schema, g.tmps, g.max_bits, g.splices);'''
assert old in s; s = s.replace(old, new)

# the helper
old = '''    /// A viewer-dependent condition's value, shifted into the variant key (a shared register).'''
new = '''    /// A per-item viewer condition over a list: its bits, once. Over a list of the register's own
    /// record (not inside another loop) whose condition reads only the item and the viewer, they
    /// are recorded at the key's start, the first time; true then (the caller records none).
    fn hoist(&mut self, roots: &HashMap<String, Root>, list: &Expr, vc: &Expr) -> Result<bool, String> {
        if !self.bits || self.per_viewer || self.mult != 1 {
            return Ok(false);
        }
        fn roots_of(e: &Expr, out: &mut Vec<String>) {
            match e {
                Expr::Dot(..) => out.push("it".into()),
                Expr::Name(n, _) => out.push(n.clone()),
                Expr::Field(o, _, _) | Expr::Not(o, _) => roots_of(o, out),
                Expr::Bin(_, l, r, _) => {
                    roots_of(l, out);
                    roots_of(r, out);
                }
                Expr::Str(..) | Expr::Int(..) | Expr::Bool(..) | Expr::None(_) | Expr::Dur(..) => {}
                _ => out.push("?".into()),
            }
        }
        let mut rs = vec![];
        roots_of(vc, &mut rs);
        if !rs.iter().all(|r| r == "it" || r == "viewer") {
            return Ok(false);
        }
        let saved = self.body.len();
        let (parent, _, _) = self.list_slot(roots, list, "it")?;
        self.body.truncate(saved);
        if parent != "%in" {
            return Ok(false);
        }
        let key = format!("{} | {}", show(list), show(vc));
        if !self.hoisted.contains(&key) {
            self.hoisted.push(key);
            let body = std::mem::take(&mut self.body);
            let (inner, l) = self.open_loop(roots, list, "it")?;
            let cv = self.cond(&inner, vc)?;
            self.bit(&cv);
            self.close_loop(l);
            let code = std::mem::replace(&mut self.body, body);
            self.prologue.push_str(&code);
        }
        Ok(true)
    }

    /// A viewer-dependent condition's value, shifted into the variant key (a shared register).'''
assert old in s; s = s.replace(old, new)

# sentence: use it
old = '''                    let t = self.tmp();
                    let (rest, vc) = Self::split_viewer(qy);
                    let list = as_list(&rest);
                    let (inner, l) = self.open_loop(roots, &list, "it")?;
                    let skip = match &vc {
                        Some(c) => {
                            let cv = self.cond(&inner, c)?;
                            self.bit(&cv);'''
new = '''                    let t = self.tmp();
                    let (rest, vc) = Self::split_viewer(qy);
                    let list = as_list(&rest);
                    let hoisted = match &vc {
                        Some(c) => self.hoist(roots, &list, c)?,
                        None => false,
                    };
                    let (inner, l) = self.open_loop(roots, &list, "it")?;
                    let skip = match &vc {
                        Some(c) => {
                            let cv = self.cond(&inner, c)?;
                            if !hoisted {
                                self.bit(&cv);
                            }'''
assert old in s; s = s.replace(old, new)

# count: use it
old = '''                let (inner, l) = self.open_loop(roots, &as_list(&rest), "it")?;
                let cv = self.cond(&inner, vc.as_ref().unwrap())?;
                self.bit(&cv);'''
new = '''                let hoisted = self.hoist(roots, &as_list(&rest), vc.as_ref().unwrap())?;
                let (inner, l) = self.open_loop(roots, &as_list(&rest), "it")?;
                let cv = self.cond(&inner, vc.as_ref().unwrap())?;
                if !hoisted {
                    self.bit(&cv);
                }'''
assert old in s; s = s.replace(old, new)
open(p, 'w', encoding='utf-8').write(s)
print('ok')
