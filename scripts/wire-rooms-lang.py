root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

edit('ast.rs', [
    ('''    Add,
    Sub,
}''', '''    Add,
    Sub,
    /// `x in list`: a row among a list argument's rows
    In,
}'''),
    ('''    Create(String, Vec<(String, Expr)>, Span),''', '''    /// `create E {…}` (`as x`: the new row, by name after)
    Create(String, Vec<(String, Expr)>, Option<String>, Span),'''),
    ('''    /// `sign out`: the request's session is deleted; its cookie cleared
    SignOut(Span),''', '''    /// `sign out`: the request's session is deleted; its cookie cleared
    SignOut(Span),
    /// `if c { … } else { … }`
    If(Expr, Vec<Stmt>, Vec<Stmt>, Span),
    /// `go screen(row)`: where the action goes when it's done
    Go(String, Expr, Span),'''),
])

edit('parse.rs', [
    ('''            return Ok(Stmt::Create(e, fs, span));''', '''            let name = if self.eat_kw("as") { Some(self.ident()?) } else { None };
            return Ok(Stmt::Create(e, fs, name, span));'''),
    ('''        if self.eat_kw("sign") {''', '''        if self.eat_kw("if") {
            let c = self.expr()?;
            let then = self.block()?;
            let otherwise = if self.eat_kw("else") {
                if self.is_kw("if") {
                    vec![self.stmt()?]
                } else {
                    self.block()?
                }
            } else {
                vec![]
            };
            return Ok(Stmt::If(c, then, otherwise, span));
        }
        if self.eat_kw("go") {
            let screen = self.ident()?;
            self.expect("(")?;
            let arg = self.expr()?;
            self.expect(")")?;
            return Ok(Stmt::Go(screen, arg, span));
        }
        if self.eat_kw("sign") {'''),
    ('''            Tok::Punct(">=") => BinOp::Ge,
            _ => return Ok(l),
        };''', '''            Tok::Punct(">=") => BinOp::Ge,
            Tok::Ident(k) if k == "in" => BinOp::In,
            _ => return Ok(l),
        };'''),
    ('''(require, create, set, touch, delete, let, sign in, sign out, for each)''', '''(require, create, set, touch, delete, let, if, go, sign in, sign out, for each)'''),
])

edit('check.rs', [
    ('''                    BinOp::Eq | BinOp::Ne => {''', '''                    BinOp::In => {
                        let ok = match b.strip_opt() {
                            Type::List(t) => a.strip_opt() == t.strip_opt(),
                            _ => false,
                        };
                        if !ok {
                            return err(*s, format!("`in` takes a row and a list of its kind, not {} and {}", a.show(), b.show()));
                        }
                        Type::Bool
                    }
                    BinOp::Eq | BinOp::Ne => {'''),
    ('''            Stmt::Create(e, fs, span) => {''', '''            Stmt::Create(e, fs, name, span) => {
                if let Some(n) = name {
                    *scope = scope.with(n, Type::Ref(e.clone()));
                }'''),
    ('''            Stmt::SignOut(span) => {''', '''            Stmt::If(c, a, b, span) => {
                if m.type_of(c, scope)? != Type::Bool {
                    return err(*span, "if takes a Bool");
                }
                check_stmts(m, a, scope, cleared)?;
                check_stmts(m, b, scope, cleared)?;
            }
            Stmt::Go(screen, arg, span) => {
                let want = m.screens.iter().find_map(|d| match d {
                    Decl::Screen { name, params, .. } if name == screen && params.len() == 1 => Some(params[0].ty.clone()),
                    _ => None,
                });
                let Some(want) = want else { return err(*span, format!("go names a screen with one row parameter, not `{screen}`")) };
                if m.type_of(arg, scope)?.strip_opt() != want.strip_opt() {
                    return err(*span, format!("{screen} takes {}", want.show()));
                }
            }
            Stmt::SignOut(span) => {'''),
    ('''        Stmt::Create(_, fs, _) => fs.iter().any(|(_, v)| expr_mentions_viewer(v)),''', '''        Stmt::Create(_, fs, _, _) => fs.iter().any(|(_, v)| expr_mentions_viewer(v)),
        Stmt::If(c, a, b, _) => expr_mentions_viewer(c) || a.iter().any(stmt_mentions_viewer) || b.iter().any(stmt_mentions_viewer),
        Stmt::Go(_, e, _) => expr_mentions_viewer(e),'''),
])

edit('netlist.rs', [
    ('''            Stmt::Create(e, _, _) => out.push((e.clone(), vec!["*".into()])),''', '''            Stmt::Create(e, _, _, _) => out.push((e.clone(), vec!["*".into()])),
            Stmt::If(_, a, b, _) => {
                collect_writes(m, a, scope, out);
                collect_writes(m, b, scope, out);
            }
            Stmt::Go(..) => {}'''),
])

edit('sql.rs', [
    ('''                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                };''', '''                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    // a list argument is bound as a JSON array of ids
                    BinOp::In => return Ok(format!("({a} IN (SELECT value FROM json_each({b})))")),
                };'''),
])
print('ok')
