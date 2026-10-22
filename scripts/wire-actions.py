# One-off: actions in the module and header (etchc).
D = 'D:/code/etch/crates/etchc/src/'

def edit(f, old, new, count=1):
    p = D + f
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == count, (f, old[:80], s.count(old))
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)

# the seq for crudspec's __seq companions: the writer's current one, read where it's used
edit('actions.rs', '''            Expr::Name(n, _) if n == "now" => Ok(("%now".into(), Shape::I64)),''',
     '''            Expr::Name(n, _) if n == "now" => Ok(("%now".into(), Shape::I64)),
            Expr::Name(n, _) if n == "@seq" => {
                let x = self.v();
                self.line(&format!("{x} = call i64 @etch_w_seq_now(ptr %w)"));
                Ok((x, Shape::I64))
            }''')
edit('actions.rs', '''                    let sel = sql::Select { sql: sel.sql, binds: sel.binds.into_iter().map(|b| match b { sql::Bind::Path(Expr::Name(n, s)) if n == "@seq" => sql::Bind::Path(Expr::Name("@seq".into(), s)), x => x }).collect() };
''', '')
edit('actions.rs', '''            let _ = writeln!(f, "  %now = call i64 @etch_w_now(ptr %w)\\n  %seqv = call i64 @etch_w_seq_now(ptr %w)");
            let body_ir = a.b.replace("%Args,", &format!("%Args_{name},")).replace("i64 @seq_value", "i64 %seqv");''',
     '''            let _ = writeln!(f, "  %now = call i64 @etch_w_now(ptr %w)");
            let body_ir = a.b.replace("%Args,", &format!("%Args_{name},"));''')

edit('llvm.rs', '''impl Module {
    fn str_const(''', '''impl Module {
    pub fn str_const_pub(&mut self, b: &[u8]) -> String {
        self.str_const(b)
    }
    fn str_const(''')
edit('llvm.rs', '''    let vl = viewer_loader(m, &mut md, &mut qid)?;
    md.funcs.push_str(&vl);''', '''    let vl = viewer_loader(m, &mut md, &mut qid)?;
    md.funcs.push_str(&vl);
    let screens = crate::actions::screen_lists(m)?;
    let acts = crate::actions::lower(m, &mut md, &mut qid, &screens)?;''')
edit('llvm.rs', '''    Ok((ll, rust_header(&md)))''', '''    let mut rs = rust_header(&md);
    // the actions: their arguments, outcomes; the registers' and screen lists' indices (delta edges name them)
    let _ = writeln!(rs, "pub const REGISTERS: &[&str] = &[{}];", md.registers.iter().map(|r| format!("{:?}", r.name)).collect::<Vec<_>>().join(", "));
    let _ = writeln!(rs, "pub const LISTS: &[&str] = &[{}];", screens.iter().map(|l| format!("{:?}", l.name)).collect::<Vec<_>>().join(", "));
    for a in &acts {
        let _ = writeln!(rs, "#[repr(C)]\\n#[derive(Clone, Copy)]\\npub struct Args_{} {{", a.name);
        for (n, s) in &a.args {
            let _ = writeln!(rs, "    pub {n}: {},", if *s == Shape::Str { "Str" } else { "i64" });
        }
        let _ = writeln!(rs, "}}");
        let _ = writeln!(rs, "pub const OUTCOMES_{}: &[&str] = &[{}];", a.name.to_uppercase(), a.outcomes.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(", "));
        let _ = writeln!(rs, "extern \\"C\\" {{\\n    pub fn etch_act_{}(ctx: *const etch_rt::abi::Ctx, db: *mut etch_rt::abi::Db, w: *mut etch_rt::abi::Writer, viewer: i64, args: *const Args_{}, d: *mut etch_rt::abi::Deltas) -> i32;\\n}}\\n", a.name, a.name);
    }
    Ok((ll, rs))''')
edit('llvm.rs', '''declare i32 @sqlite3_reset(ptr)
"#;''', '''declare i32 @sqlite3_reset(ptr)
declare void @etch_exec(ptr)
declare void @etch_done(ptr, i32)
declare void @etch_delta(ptr, i32, i64, i64, i64)
declare i64 @etch_w_now(ptr)
declare i64 @etch_w_seq(ptr)
declare i64 @etch_w_seq_now(ptr)
declare i64 @etch_w_id(ptr, ptr, ptr, i64)
declare void @etch_w_token(ptr, i64, ptr)
"#;''')
edit('main.rs', 'mod check;\n', 'mod actions;\nmod check;\n')
print('ok')
