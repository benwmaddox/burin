import io

R = '/mnt/d/code/etch/'

def edit(p, pairs):
    s = io.open(R + p, newline='').read()
    for a, b in pairs:
        assert s.count(a) == 1, (p, a[:90], s.count(a))
        s = s.replace(a, b)
    io.open(R + p, 'w', newline='').write(s)

edit('crates/etchc/src/ast.rs', [
    ('''    Extern { name: String, params: Vec<Param>, ret: Type, effect: bool, span: Span },''',
     '''    /// `from "symbol"`: its name in the C ABI (else its own name); the standard library's are
    /// the runtime's `etch_std_…`.
    Extern { name: String, params: Vec<Param>, ret: Type, effect: bool, symbol: String, span: Span },'''),
])

edit('crates/etchc/src/parse.rs', [
    ('''                let ret = self.ty()?;
                Ok(Decl::Extern { name, params, ret, effect, span })''', '''                let ret = self.ty()?;
                let symbol = if self.eat_kw("from") { self.string()? } else { name.clone() };
                Ok(Decl::Extern { name, params, ret, effect, symbol, span })'''),
    ('''    let files: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let mut decls = vec![];
    for (i, p) in paths.iter().enumerate() {
        let src = std::fs::read_to_string(p).map_err(|e| (files.clone(), Error { span: Span { file: i as u32, line: 0 }, msg: e.to_string() }))?;
        decls.extend(parse_file(&src, i as u32).map_err(|e| (files.clone(), e))?);
    }
    Ok(App { files, decls })''', '''    let mut files: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let mut decls = vec![];
    for (i, p) in paths.iter().enumerate() {
        let src = std::fs::read_to_string(p).map_err(|e| (files.clone(), Error { span: Span { file: i as u32, line: 0 }, msg: e.to_string() }))?;
        decls.extend(parse_file(&src, i as u32).map_err(|e| (files.clone(), e))?);
    }
    files.push(PRELUDE_NAME.into());
    decls.extend(prelude(files.len() as u32 - 1).map_err(|e| (files.clone(), e))?);
    Ok(App { files, decls })
}

/// The standard library every app has (lang/std.etch, built into the compiler).
pub const PRELUDE: &str = include_str!("../../../lang/std.etch");
pub const PRELUDE_NAME: &str = "lang/std.etch";

/// The standard library's declarations, as file `file`.
pub fn prelude(file: u32) -> R<Vec<Decl>> {
    parse_file(PRELUDE, file)'''),
])

edit('crates/etchc/src/check.rs', [
    ('''    pub fn viewer_entity(&self) -> Option<&str> {''', '''    /// An `extern fn`'s name in the C ABI.
    pub fn extern_symbol(&self, f: &str) -> String {
        match self.externs.get(f) {
            Some(Decl::Extern { symbol, .. }) => symbol.clone(),
            _ => f.to_string(),
        }
    }

    pub fn viewer_entity(&self) -> Option<&str> {'''),
])

L = 'crates/etchc/src/llvm.rs'
edit(L, [
    ('''                let (list, n) = self.extern_args(roots, f, a)?;
                let v = self.v();
                self.line(&format!("{v} = call i64 @{f}({list})"));''', '''                let (list, n) = self.extern_args(roots, f, a)?;
                let v = self.v();
                self.line(&format!("{v} = call i64 @{}({list})", self.m.extern_symbol(f)));'''),
    ('''                        self.line(&format!("call void @{f}({list}{sep}ptr {r})"));''', '''                        self.line(&format!("call void @{}({list}{sep}ptr {r})", self.m.extern_symbol(f)));'''),
    ('''        let Some(Decl::Extern { params, ret, .. }) = m.externs.get(name) else { continue };''', '''        let Some(Decl::Extern { params, ret, symbol, .. }) = m.externs.get(name) else { continue };'''),
    ('''        let _ = writeln!(out, "declare {r} @{name}({})", args.join(", "));''', '''        let _ = writeln!(out, "declare {r} @{symbol}({})", args.join(", "));'''),
])

A = 'crates/etchc/src/actions.rs'
edit(A, [
    ('''                    self.line(&format!("{o} = call ptr @etch_w_ext_out(ptr %w)\\n  call void @{f}({}{sep}ptr {o})", list.join(", ")));''',
     '''                    self.line(&format!("{o} = call ptr @etch_w_ext_out(ptr %w)\\n  call void @{}({}{sep}ptr {o})", self.m.extern_symbol(f), list.join(", ")));'''),
    ('''                    self.line(&format!("{r} = call i64 @{f}({})", list.join(", ")));''', '''                    self.line(&format!("{r} = call i64 @{}({})", self.m.extern_symbol(f), list.join(", ")));'''),
])

S = 'crates/etchc/src/sql.rs'
edit(S, [
    ('''            Expr::Call(f, args, _) if self.m.externs.contains_key(f) => {
                if args.iter().any(reads_row) {''', '''            // a standard function with a SQL form (lang/std_sql.rs): SQLite runs it
            Expr::Call(f, args, _) if self.m.externs.contains_key(f) && std_sql(self.m, f).is_some() => {
                let mut form = std_sql(self.m, f).unwrap().to_string();
                // (the highest first: $1 isn't a prefix of $10)
                for (i, a) in args.iter().enumerate().rev() {
                    let x = self.expr(a)?;
                    form = form.replace(&format!("${}", i + 1), &x);
                }
                form
            }
            Expr::Call(f, args, _) if self.m.externs.contains_key(f) => {
                if args.iter().any(reads_row) {'''),
])
s = io.open(R + S, newline='').read()
s += '''
include!("../../../lang/std_sql.rs");

/// A standard library function's SQL form (only the prelude's: an app's own extern of the same
/// name couldn't exist, it would be declared twice).
fn std_sql(m: &Model, f: &str) -> Option<&'static str> {
    match m.externs.get(f) {
        Some(Decl::Extern { symbol, .. }) if symbol.starts_with("etch_std_") => STD_SQL.iter().find(|(n, _)| *n == f).map(|(_, s)| *s),
        _ => None,
    }
}
'''
io.open(R + S, 'w', newline='').write(s)

T = 'crates/etchc/src/tests.rs'
edit(T, [
    ('''    let decls = parse::parse_file(&src, 0).map_err(|e| format!("parse: {}", e.msg))?;
    let app = App { files: vec!["t.etch".into()], decls };''', '''    let mut decls = parse::parse_file(&src, 0).map_err(|e| format!("parse: {}", e.msg))?;
    decls.extend(parse::prelude(1).map_err(|e| format!("the prelude: {}", e.msg))?);
    let app = App { files: vec!["t.etch".into(), parse::PRELUDE_NAME.into()], decls };'''),
])
s = io.open(R + T, newline='').read()
s += '''
#[test]
fn the_standard_library_is_in_every_app() {
    verdict(r#"fragment p() <<<{{for p in recent()}}{{if contains(p.body, "x") and length(p.body) > 3}}{{truncate(p.body, 10)}} {{plural(length(p.body), "char", "chars")}}{{end}}{{end}}>>>"#).unwrap();
    let e = verdict("extern fn length(t: Text): Int").unwrap_err();
    assert!(e.contains("declared twice"), "{e}");
}
'''
io.open(R + T, 'w', newline='').write(s)
print('ok')
