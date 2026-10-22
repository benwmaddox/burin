root = '/mnt/d/code/etch/crates/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

# ---- screens: a response type and a browser cache lifetime
edit('etchc/src/ast.rs', [
    ('''    Screen { name: String, params: Vec<Param>, route: String, cold: bool, public: bool, requires: Vec<Require>, cookies: Vec<(String, Expr)>, body: Template, span: Span },''',
     '''    /// `type "…"`: the response's content type (HTML when absent); `max_age N`: how long a browser
    /// may keep it (seconds; private)
    Screen { name: String, params: Vec<Param>, route: String, cold: bool, public: bool, ctype: Option<String>, max_age: Option<i64>, requires: Vec<Require>, cookies: Vec<(String, Expr)>, body: Template, span: Span },'''),
])
edit('etchc/src/parse.rs', [
    ('''                let cold = self.eat_kw("cold");
                let public = self.eat_kw("public");''', '''                let (mut cold, mut public, mut ctype, mut max_age) = (false, false, None, None);
                loop {
                    if self.eat_kw("cold") {
                        cold = true;
                    } else if self.eat_kw("public") {
                        public = true;
                    } else if self.eat_kw("type") {
                        ctype = Some(self.string()?);
                    } else if self.eat_kw("max_age") {
                        match self.next() {
                            Tok::Int(n) => max_age = Some(n),
                            _ => return self.err("expected a number of seconds after max_age"),
                        }
                    } else {
                        break;
                    }
                }'''),
    ('''                Ok(Decl::Screen { name, params, route, cold, public, requires, cookies, body: self.template()?, span })''',
     '''                Ok(Decl::Screen { name, params, route, cold, public, ctype, max_age, requires, cookies, body: self.template()?, span })'''),
])
edit('etchc/src/glue.rs', [
    ('''        let (params, requires, cookies, body, public, cold) = match d {
            Decl::Screen { params, requires, cookies, body, public, cold, .. } => (params, requires, cookies, body, *public, *cold),
            _ => continue,
        };''', '''        let (params, requires, cookies, body, public, cold, ctype, max_age) = match d {
            Decl::Screen { params, requires, cookies, body, public, cold, ctype, max_age, .. } => (params, requires, cookies, body, *public, *cold, ctype.clone(), *max_age),
            _ => continue,
        };'''),
    ('''        let mut headers = String::from("Content-Type: text/html; charset=utf-8\\\\r\\\\n");''', '''        let mut headers = format!("Content-Type: {}\\\\r\\\\n", ctype.unwrap_or_else(|| "text/html; charset=utf-8".into()));
        if let Some(n) = max_age {
            headers.push_str(&format!("Cache-Control: private, max-age={n}\\\\r\\\\n"));
        }'''),
])

# ---- builtins: upper, choose, hash
edit('etchc/src/check.rs', [
    ('''            "present" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Bool)
            }''', '''            "present" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Bool)
            }
            "upper" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Text)
            }
            // one of a constant list of texts, picked by the CRC-32 of a key's text
            "choose" => {
                arity(2)?;
                match &args[0] {
                    Expr::Name(n, _) if matches!(self.consts.get(n), Some(Decl::Const { value: Expr::List(..), .. })) => {}
                    _ => return err(s, "choose takes a constant list, then a key"),
                }
                arg(1, Type::Text)?;
                Ok(Type::Text)
            }
            // a password's hash (made off the writer, in an action only)
            "hash" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Password)
            }'''),
])
edit('etchc/src/llvm.rs', [
    ('''                "first_word" | "initials" | "lower" => {''', '''                "choose" => {
                    let items: Vec<String> = match &args[0] {
                        Expr::Name(n, _) => match self.m.consts.get(n) {
                            Some(Decl::Const { value: Expr::List(xs, _), .. }) => xs.iter().map(|x| self.constant(roots, x).ok_or("choose's list holds texts")).collect::<Result<_, _>>()?,
                            _ => return Err("choose takes a constant list".into()),
                        },
                        _ => return Err("choose takes a constant list".into()),
                    };
                    let t = self.tmp();
                    self.text(roots, &args[1], &t, false)?;
                    self.flush(&t);
                    if !self.bits {
                        let i = self.v();
                        self.line(&format!("{i} = call i64 @etch_crc_mod(ptr {t}, i64 {})", items.len()));
                        let done = self.label("chosen");
                        let labels: Vec<String> = (0..items.len()).map(|_| self.label("choice")).collect();
                        let cases: String = labels.iter().enumerate().map(|(k, l)| format!("i64 {k}, label %{l} ")).collect();
                        self.line(&format!("switch i64 {i}, label %{done} [ {cases}]"));
                        for (k, l) in labels.iter().enumerate() {
                            self.block(l);
                            let b = if esc { esc_bytes(items[k].as_bytes()) } else { items[k].clone().into_bytes() };
                            let g = self.md.str_const(&b);
                            self.line(&format!("call void @etch_put(ptr {out}, ptr {g}, i64 {})", b.len()));
                            self.line(&format!("br label %{done}"));
                        }
                        self.block(&done);
                    }
                    self.untmp();
                }
                "first_word" | "initials" | "lower" | "upper" => {'''),
    ('''declare void @etch_w_sign_out(ptr)''', '''declare void @etch_w_sign_out(ptr)
declare i64 @etch_crc_mod(ptr, i64)
declare void @etch_upper(ptr, ptr, i64)'''),
])

# ---- hash(): the password hashed on the request's thread, a hidden argument to the writer
edit('etchc/src/actions.rs', [
    ('''    /// arguments that are lists of rows: from repeated fields, as JSON arrays of ids
    pub list_args: Vec<String>,
}''', '''    /// arguments that are lists of rows: from repeated fields, as JSON arrays of ids
    pub list_args: Vec<String>,
    /// each `hash(text)`: (its index, the text's argument); the runtime hashes it off the writer
    /// (`__hashed_<i>`, empty for empty text)
    pub hashes: Vec<(usize, String)>,
}'''),
    ('''            Expr::Call(f, a, _) if f == "token" => {''', '''            Expr::Call(f, a, _) if f == "hash" => {
                let arg = match a.first() {
                    Some(Expr::Name(n, _)) if self.args.iter().any(|(x, s)| x == n && *s == Shape::Str) => n.clone(),
                    _ => return Err("hash takes a text argument".into()),
                };
                let i = match self.hashes.iter().position(|(_, x)| *x == arg) {
                    Some(i) => i,
                    None => {
                        self.hashes.push((self.hashes.len(), arg));
                        self.hashes.len() - 1
                    }
                };
                self.value(&Expr::Name(format!("__hashed_{i}"), Span::default()))
            }
            Expr::Call(f, a, _) if f == "token" => {'''),
    ('''    /// the registers ('r') and screen lists ('l') a write's precise edges reached
    touched: Vec<(char, usize)>,
}''', '''    /// the registers ('r') and screen lists ('l') a write's precise edges reached
    touched: Vec<(char, usize)>,
    hashes: Vec<(usize, String)>,
}'''),
    ('''verifies: vec![], allocas: String::new(), touched: vec![] };''', '''verifies: vec![], allocas: String::new(), touched: vec![], hashes: vec![] };'''),
    ('''            let mut hid = vec![];
            hidden(body, &mut hid, &mut 0);''', '''            let mut hid = vec![];
            hidden(body, &mut hid, &mut 0);
            // each distinct hash(argument), in order of appearance
            fn hashed(body: &[Stmt], out: &mut Vec<String>) {
                fn ex(e: &Expr, out: &mut Vec<String>) {
                    match e {
                        Expr::Call(f, a, _) if f == "hash" => {
                            if let Some(Expr::Name(n, _)) = a.first() {
                                if !out.contains(n) {
                                    out.push(n.clone());
                                }
                            }
                        }
                        Expr::Call(_, a, _) => a.iter().for_each(|x| ex(x, out)),
                        Expr::Bin(_, l, r, _) => {
                            ex(l, out);
                            ex(r, out);
                        }
                        _ => {}
                    }
                }
                for s in body {
                    match s {
                        Stmt::Set(_, _, v, _) => ex(v, out),
                        Stmt::Create(_, fs, _, _) => fs.iter().for_each(|(_, v)| ex(v, out)),
                        Stmt::ForEach(_, _, inner, _) => hashed(inner, out),
                        Stmt::If(_, a, b, _) => {
                            hashed(a, out);
                            hashed(b, out);
                        }
                        _ => {}
                    }
                }
            }
            let mut hs = vec![];
            hashed(body, &mut hs);
            hid.extend((0..hs.len()).map(|i| format!("__hashed_{i}")));'''),
    ('''            let (outcomes, edges, patches, verifies) = (a.outcomes.clone(), a.edges.clone(), a.patches.clone(), a.verifies.clone());''',
     '''            let (outcomes, edges, patches, verifies, hashes) = (a.outcomes.clone(), a.edges.clone(), a.patches.clone(), a.verifies.clone(), a.hashes.clone());'''),
    ('''            infos.push(ActInfo { name: name.clone(), args, outcomes, edges, public: *public, verifies, list_args });''',
     '''            infos.push(ActInfo { name: name.clone(), args, outcomes, edges, public: *public, verifies, list_args, hashes });'''),
    # a change to a viewer's row: precise (that viewer, at its id)
    ('''        // (a new row changes no one's record)
        if !created && hit(&self.md.viewer_reads.clone()) {
            self.delta(15, 0, "0", "0");
            self.note(format!("{verb} {ent}.{} → the viewers' records: FALLBACK (all read again, the screens rebuilt)", cols.join(",")));
        }''', '''        // (a new row changes no one's record)
        if !created && hit(&self.md.viewer_reads.clone()) {
            let own = self.m.viewer_entity() == Some(ent);
            match vals.get("id").cloned().filter(|_| own) {
                // the viewer's own row: its record, its registers and the pages it can see
                Some(id) => {
                    self.delta(15, 0, &id, "0");
                    self.note(format!("{verb} {ent}.{} → that viewer: READ (their record, their own registers, the pages they can see)", cols.join(",")));
                }
                None => {
                    self.delta(15, 0, "-9223372036854775808", "0");
                    self.note(format!("{verb} {ent}.{} → the viewers' records: FALLBACK (all read again, the screens rebuilt)", cols.join(",")));
                }
            }
        }'''),
])
edit('etchc/src/glue.rs', [
    ('''    let _ = writeln!(s, "            _ => {{}}\\n        }}\\n    }}");
    // where "/" goes''', '''    let _ = writeln!(s, "            _ => {{}}\\n        }}");
    // each hash(text): hashed here (slow by design), off the writer
    let _ = writeln!(s, "        match name {{");
    for a in acts.iter().filter(|a| !a.hashes.is_empty()) {
        let _ = writeln!(s, "            {:?} => {{", a.name);
        for (i, arg) in &a.hashes {
            let _ = writeln!(s, "                let t = etch_rt::app::field(form, {arg:?}).unwrap_or(\\"\\").to_string();\\n                form.push((\\"__hashed_{i}\\".to_string(), if t.is_empty() {{ String::new() }} else {{ etch_rt::pass::hash(&t) }}));");
        }
        let _ = writeln!(s, "            }}");
    }
    let _ = writeln!(s, "            _ => {{}}\\n        }}\\n    }}");
    // where "/" goes'''),
])

# ---- runtime
edit('etch-rt/src/abi.rs', [
    ('''/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_lower(''', '''/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_upper(out: *mut Vec<u8>, t: *mut Vec<u8>, esc: i64) {
    put(buf(out), &text(t).to_uppercase(), esc);
}

/// `choose`: the CRC-32 of a text, modulo n (Campfire picks an avatar's color this way).
/// # Safety: a live Vec.
#[no_mangle]
pub unsafe extern "C" fn etch_crc_mod(t: *mut Vec<u8>, n: i64) -> i64 {
    (crc32fast::hash(&*t) as i64).rem_euclid(n.max(1))
}

/// # Safety: live Vecs.
#[no_mangle]
pub unsafe extern "C" fn etch_lower('''),
])
edit('etch-rt/src/app.rs', [
    ('''                15 => {
                    stats.cleared.fetch_add(1, Ordering::Relaxed);
                    viewers.clear();
                    reload.extend(kept.keys().copied());
                    for s in &mut next.screens {
                        s.clear();
                    }
                }''', '''                15 if key == i64::MIN => {
                    stats.cleared.fetch_add(1, Ordering::Relaxed);
                    viewers.clear();
                    reload.extend(kept.keys().copied());
                    for s in &mut next.screens {
                        s.clear();
                    }
                }
                // one viewer's row changed: their record, their own registers, and every kept page
                // they can see (their variants may change)
                15 => {
                    viewers.remove(&key);
                    reload.extend(kept.keys().filter(|(_, u)| *u == key).copied());
                    for s in 0..G::SCREENS {
                        for (k, inst) in next.screens[s].iter() {
                            if inst.audience.binary_search(&key).is_ok() {
                                rescreen.insert((s, *k));
                            }
                        }
                    }
                }'''),
    ('''        let (mut shared, mut relist, mut dropped, mut audiences): (HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>) = Default::default();''',
     '''        let (mut shared, mut relist, mut dropped, mut audiences): (HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>, HashSet<(usize, i64)>) = Default::default();
        let mut rescreen: HashSet<(usize, i64)> = HashSet::new();'''),
    ('''        // audiences that may have changed: read again;''', '''        // pages rebuilt in full (a viewer's row changed)
        for (s, key) in rescreen {
            if next.screens[s].contains_key(&key) {
                build_screen::<G>(&c, &ctx, &mut next, &mut viewers, s, key);
            }
        }
        // audiences that may have changed: read again;'''),
])
print('ok')
