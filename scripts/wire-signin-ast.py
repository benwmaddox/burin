import re
root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:60])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

edit('ast.rs', [
    ('''    Duration,
    /// a reference to a row''', '''    Duration,
    /// a password's hash (PHC text): never shown; only `authenticates(hash, text)` reads it
    Password,
    /// a reference to a row'''),
    ('''            Type::Duration => "Duration".into(),''', '''            Type::Duration => "Duration".into(),
            Type::Password => "Password".into(),'''),
    ('''    /// `delete x`: the row goes; rows referencing it must have gone first (the checker proves it)
    Delete(Expr, Span),''', '''    /// `delete x`: the row goes; rows referencing it must have gone first (the checker proves it)
    Delete(Expr, Span),
    /// `let x = e`: a value read once (a row: `first(…)`), by name after
    Let(String, Expr, Span),
    /// `sign in u`: a new session for u (the viewer declaration's entity); its cookie is set
    SignIn(Expr, Span),
    /// `sign out`: the request's session is deleted; its cookie cleared
    SignOut(Span),'''),
    ('''    /// `cold`: rendered per request, outside the fast path
    Screen { name: String, params: Vec<Param>, route: String, cold: bool, requires: Vec<Require>, cookies: Vec<(String, Expr)>, body: Template, span: Span },
    Action { name: String, params: Vec<Param>, body: Vec<Stmt>, span: Span },''', '''    /// `cold`: rendered per request, outside the fast path; `public`: for anyone (no viewer, no
    /// parameters: rendered per request)
    Screen { name: String, params: Vec<Param>, route: String, cold: bool, public: bool, requires: Vec<Require>, cookies: Vec<(String, Expr)>, body: Template, span: Span },
    /// `public`: run without a viewer (signing in)
    Action { name: String, params: Vec<Param>, public: bool, body: Vec<Stmt>, span: Span },
    /// `home <screen>(<row>)`: where "/" goes for a viewer
    Home { screen: String, arg: Expr, span: Span },'''),
])

edit('parse.rs', [
    ('''                "Duration" => Type::Duration,''', '''                "Duration" => Type::Duration,
                "Password" => Type::Password,'''),
    ('''                let cold = self.eat_kw("cold");''', '''                let cold = self.eat_kw("cold");
                let public = self.eat_kw("public");'''),
    ('''                Ok(Decl::Screen { name, params, route, cold, requires, cookies, body: self.template()?, span })''',
     '''                Ok(Decl::Screen { name, params, route, cold, public, requires, cookies, body: self.template()?, span })'''),
    ('''                let params = self.params()?;
                let body = self.block()?;
                Ok(Decl::Action { name, params, body, span })''', '''                let params = self.params()?;
                let public = self.eat_kw("public");
                let body = self.block()?;
                Ok(Decl::Action { name, params, public, body, span })
            }
            "home" => {
                let screen = self.ident()?;
                self.expect("(")?;
                let arg = self.expr()?;
                self.expect(")")?;
                Ok(Decl::Home { screen, arg, span })'''),
    ('''        if self.eat_kw("delete") {
            return Ok(Stmt::Delete(self.expr()?, span));
        }''', '''        if self.eat_kw("delete") {
            return Ok(Stmt::Delete(self.expr()?, span));
        }
        if self.eat_kw("let") {
            let n = self.ident()?;
            self.expect("=")?;
            return Ok(Stmt::Let(n, self.expr()?, span));
        }
        if self.eat_kw("sign") {
            if self.eat_kw("out") {
                return Ok(Stmt::SignOut(span));
            }
            self.expect_kw("in")?;
            return Ok(Stmt::SignIn(self.expr()?, span));
        }'''),
    ('''(require, create, set, touch, delete, for each)''', '''(require, create, set, touch, delete, let, sign in, sign out, for each)'''),
])
print('ok')
