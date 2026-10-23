//! The parser: every .burin file of an app into declarations. Code is tokenized; a template
//! (`<<< … >>>`) is taken as raw bytes, then split into text and `{{ … }}` holes, whose code is
//! tokenized again.

use crate::ast::*;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Int(i64),
    /// a duration in milliseconds
    Dur(i64),
    Punct(&'static str),
    Template(String),
    Eof,
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
    line: u32,
    file: u32,
}

pub struct Error {
    pub span: Span,
    pub msg: String,
}

type R<T> = Result<T, Error>;

const PUNCTS: &[&str] = &["==", "!=", "<=", ">=", "->", "..", "<<", ">>", "+%", "-%", "*%", "&", "|", "^", "{", "}", "(", ")", "[", "]", ",", ":", ".", "=", "<", ">", "+", "-", "?", "*", "/", "%"];

impl<'a> Lexer<'a> {
    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(Error { span: Span { file: self.file, line: self.line }, msg: msg.into() })
    }

    fn tokens(mut self) -> R<Vec<(Tok, Span)>> {
        let mut out = vec![];
        loop {
            // spaces and comments
            while self.i < self.s.len() {
                let c = self.s[self.i];
                if c == b'\n' {
                    self.line += 1;
                    self.i += 1;
                } else if c.is_ascii_whitespace() {
                    self.i += 1;
                } else if self.s[self.i..].starts_with(b"//") {
                    while self.i < self.s.len() && self.s[self.i] != b'\n' {
                        self.i += 1;
                    }
                } else {
                    break;
                }
            }
            let span = Span { file: self.file, line: self.line };
            if self.i >= self.s.len() {
                out.push((Tok::Eof, span));
                return Ok(out);
            }
            let c = self.s[self.i];
            if self.s[self.i..].starts_with(b"<<<") {
                self.i += 3;
                let start = self.i;
                // the end is the last three of the first run of '>' that has at least three:
                // `…list>>>>` ends a template whose text ends in '>'
                loop {
                    if self.i >= self.s.len() {
                        return self.err("a template (<<<) isn't closed with >>>");
                    }
                    if self.s[self.i] == b'>' {
                        let mut j = self.i;
                        while j < self.s.len() && self.s[j] == b'>' {
                            j += 1;
                        }
                        if j - self.i >= 3 {
                            let text = std::str::from_utf8(&self.s[start..j - 3]).unwrap().to_string();
                            self.line += text.matches('\n').count() as u32;
                            self.i = j;
                            out.push((Tok::Template(text), span));
                            break;
                        }
                        self.i = j;
                    } else {
                        self.i += 1;
                    }
                }
                continue;
            }
            if c == b'"' {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    if self.i >= self.s.len() || self.s[self.i] == b'\n' {
                        return self.err("a string isn't closed");
                    }
                    let c = self.s[self.i];
                    self.i += 1;
                    match c {
                        b'"' => break,
                        b'\\' => {
                            let e = self.s.get(self.i).copied().unwrap_or(b' ');
                            self.i += 1;
                            v.push(match e {
                                b'n' => b'\n',
                                b't' => b'\t',
                                b'"' => b'"',
                                b'\\' => b'\\',
                                _ => return self.err(format!("unknown escape \\{}", e as char)),
                            });
                        }
                        c => v.push(c),
                    }
                }
                out.push((Tok::Str(String::from_utf8(v).map_err(|_| Error { span, msg: "a string isn't UTF-8".into() })?), span));
                continue;
            }
            // a hexadecimal number (`0xEDB88320`)
            if self.s[self.i..].starts_with(b"0x") {
                let start = self.i + 2;
                self.i = start;
                while self.i < self.s.len() && self.s[self.i].is_ascii_hexdigit() {
                    self.i += 1;
                }
                let n = i64::from_str_radix(std::str::from_utf8(&self.s[start..self.i]).unwrap(), 16).or_else(|_| self.err("a hexadecimal number too big for 64 bits"))?;
                out.push((Tok::Int(n), span));
                continue;
            }
            if c.is_ascii_digit() {
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                    self.i += 1;
                }
                let n: i64 = std::str::from_utf8(&self.s[start..self.i]).unwrap().parse().unwrap();
                // a unit makes it a duration
                let rest = &self.s[self.i..];
                let unit = [("ms", 1), ("min", 60_000), ("s", 1000), ("h", 3_600_000), ("d", 86_400_000)].iter().find(|(u, _)| {
                    rest.starts_with(u.as_bytes()) && !rest.get(u.len()).map(|c| c.is_ascii_alphanumeric() || *c == b'_').unwrap_or(false)
                });
                if let Some((u, k)) = unit {
                    self.i += u.len();
                    out.push((Tok::Dur(n * k), span));
                } else {
                    out.push((Tok::Int(n), span));
                }
                continue;
            }
            if c.is_ascii_alphabetic() || c == b'_' {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
                    self.i += 1;
                }
                out.push((Tok::Ident(std::str::from_utf8(&self.s[start..self.i]).unwrap().to_string()), span));
                continue;
            }
            match PUNCTS.iter().find(|p| self.s[self.i..].starts_with(p.as_bytes())) {
                Some(p) => {
                    self.i += p.len();
                    out.push((Tok::Punct(p), span));
                }
                None => return self.err(format!("unexpected character {:?}", c as char)),
            }
        }
    }
}

struct Parser {
    toks: Vec<(Tok, Span)>,
    i: usize,
    file: u32,
}

fn is_entity_name(s: &str) -> bool {
    s.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].0
    }
    fn peek_at(&self, k: usize) -> &Tok {
        &self.toks[(self.i + k).min(self.toks.len() - 1)].0
    }
    fn span(&self) -> Span {
        self.toks[self.i].1
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.i].0.clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }
    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(Error { span: self.span(), msg: msg.into() })
    }
    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }
    fn is_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Tok::Ident(q) if q == k)
    }
    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.next();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        if self.is_kw(k) {
            self.next();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, p: &str) -> R<()> {
        if self.eat(p) {
            Ok(())
        } else {
            self.err(format!("expected `{p}`, found {}", show(self.peek())))
        }
    }
    fn expect_kw(&mut self, k: &str) -> R<()> {
        if self.eat_kw(k) {
            Ok(())
        } else {
            self.err(format!("expected `{k}`, found {}", show(self.peek())))
        }
    }
    fn ident(&mut self) -> R<String> {
        match self.next() {
            Tok::Ident(s) => Ok(s),
            t => {
                self.i -= 1;
                self.err(format!("expected a name, found {}", show(&t)))
            }
        }
    }
    fn string(&mut self) -> R<String> {
        match self.next() {
            Tok::Str(s) => Ok(s),
            t => {
                self.i -= 1;
                self.err(format!("expected a string, found {}", show(&t)))
            }
        }
    }

    fn ty(&mut self) -> R<Type> {
        let t = if self.eat_kw("list") {
            self.expect_kw("of")?;
            Type::List(Box::new(self.ty()?))
        } else {
            let n = self.ident()?;
            match n.as_str() {
                "Text" => Type::Text,
                "Int" => Type::Int,
                "Bool" => Type::Bool,
                "Time" => Type::Time,
                "Duration" => Type::Duration,
                "Password" => Type::Password,
                "Html" => Type::Html,
                _ if is_entity_name(&n) => Type::Ref(n),
                _ => return self.err(format!("unknown type {n}")),
            }
        };
        Ok(if self.eat("?") { Type::Opt(Box::new(t)) } else { t })
    }

    fn params(&mut self) -> R<Vec<Param>> {
        self.expect("(")?;
        let mut ps = vec![];
        while !self.eat(")") {
            let name = self.ident()?;
            self.expect(":")?;
            ps.push(Param { name, ty: self.ty()? });
            if !self.eat(",") {
                self.expect(")")?;
                break;
            }
        }
        Ok(ps)
    }

    fn template(&mut self) -> R<Template> {
        let span = self.span();
        match self.next() {
            Tok::Template(raw) => Ok(Template { nodes: parse_template(&raw, span)?, span }),
            t => {
                self.i -= 1;
                self.err(format!("expected a template (<<< … >>>), found {}", show(&t)))
            }
        }
    }

    fn decl(&mut self) -> R<Decl> {
        let span = self.span();
        let kw = self.ident()?;
        match kw.as_str() {
            "config" => {
                let name = self.ident()?;
                self.expect(":")?;
                Ok(Decl::Config { name, ty: self.ty()?, span })
            }
            "entity" => {
                let name = self.ident()?;
                self.expect("{")?;
                let mut fields = vec![];
                while !self.eat("}") {
                    let f = self.ident()?;
                    self.expect(":")?;
                    let ty = self.ty()?;
                    let max = if self.eat_kw("max") {
                        match self.next() {
                            Tok::Int(n) => Some(n),
                            _ => return self.err("expected a number after max"),
                        }
                    } else {
                        None
                    };
                    fields.push(Field { name: f, ty, max });
                }
                Ok(Decl::Entity(Entity { name, fields, span }))
            }
            "viewer" => {
                let entity = self.ident()?;
                self.expect_kw("from")?;
                self.expect_kw("cookie")?;
                let cookie = self.ident()?;
                self.expect("=")?;
                let via_entity = self.ident()?;
                self.expect(".")?;
                let via_field = self.ident()?;
                self.expect("->")?;
                self.expect(".")?;
                let to_field = self.ident()?;
                Ok(Decl::Viewer { entity, cookie, via_entity, via_field, to_field, span })
            }
            "extern" => {
                let effect = self.eat_kw("effect");
                self.expect_kw("fn")?;
                let name = self.ident()?;
                let params = self.params()?;
                self.expect(":")?;
                let ret = self.ty()?;
                let symbol = if self.eat_kw("from") { self.string()? } else { name.clone() };
                Ok(Decl::Extern { name, params, ret, effect, symbol, span })
            }
            "fn" => {
                let name = self.ident()?;
                let params = self.params()?;
                self.expect(":")?;
                let ret = self.ty()?;
                if self.is("{") {
                    return Ok(Decl::Helper { name, params, ret, body: self.hblock()?, span });
                }
                self.expect("=")?;
                Ok(Decl::Fn { name, params, ret, body: self.expr()?, span })
            }
            "view" => {
                let name = self.ident()?;
                let params = self.params()?;
                self.expect("=")?;
                let e = self.expr()?;
                let query = match e {
                    Expr::Query(q) => *q,
                    other => Query { source: Source::Expr(other), filter: None, order: vec![], limit: None, window_last: None, span },
                };
                Ok(Decl::View { name, params, query, span })
            }
            "const" => {
                let name = self.ident()?;
                self.expect("=")?;
                Ok(Decl::Const { name, value: self.expr()?, span })
            }
            "fragment" => {
                let name = self.ident()?;
                let params = self.params()?;
                let per_viewer = if self.eat_kw("per") {
                    self.expect_kw("viewer")?;
                    true
                } else {
                    false
                };
                if self.is_kw("varies") {
                    return self.err("a cached fragment is the same for everyone (it can't vary by the viewer): mark it `per viewer` to render it per request");
                }
                Ok(Decl::Fragment { name, params, per_viewer, body: self.template()?, span })
            }
            "screen" => {
                let name = self.ident()?;
                let params = self.params()?;
                self.expect_kw("at")?;
                let route = self.string()?;
                let (mut cold, mut public, mut ctype, mut max_age) = (false, false, None, None);
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
                }
                let mut requires = vec![];
                let mut cookies = vec![];
                let mut watch = None;
                loop {
                    if self.eat_kw("require") {
                        requires.push(self.require()?);
                    } else if self.eat_kw("watch") {
                        watch = Some(self.ident()?);
                    } else if self.eat_kw("cookie") {
                        let n = self.ident()?;
                        self.expect("=")?;
                        cookies.push((n, self.expr()?));
                    } else {
                        break;
                    }
                }
                Ok(Decl::Screen { name, params, route, cold, public, ctype, max_age, requires, cookies, watch, body: self.template()?, span })
            }
            "action" => {
                let name = self.ident()?;
                let params = self.params()?;
                let public = self.eat_kw("public");
                let body = self.block()?;
                Ok(Decl::Action { name, params, public, body, span })
            }
            "home" => {
                let screen = self.ident()?;
                self.expect("(")?;
                let arg = self.expr()?;
                self.expect(")")?;
                Ok(Decl::Home { screen, arg, span })
            }
            _ => Err(Error { span, msg: format!("expected a declaration (entity, fn, view, screen, …), found `{kw}`") }),
        }
    }

    fn require(&mut self) -> R<Require> {
        let cond = self.expr()?;
        self.expect_kw("else")?;
        let otherwise = if self.eat_kw("go") {
            Else::Go(self.string()?)
        } else if self.eat_kw("unsupported") {
            Else::Unsupported
        } else {
            Else::Outcome(self.ident()?)
        };
        Ok(Require { cond, otherwise })
    }

    fn block(&mut self) -> R<Vec<Stmt>> {
        self.expect("{")?;
        let mut out = vec![];
        while !self.eat("}") {
            out.push(self.stmt()?);
        }
        Ok(out)
    }

    fn stmt(&mut self) -> R<Stmt> {
        let span = self.span();
        if self.eat_kw("require") {
            return Ok(Stmt::Require(self.require()?));
        }
        if self.eat_kw("create") {
            let e = self.ident()?;
            self.expect("{")?;
            let mut fs = vec![];
            while !self.eat("}") {
                let f = self.ident()?;
                self.expect(":")?;
                fs.push((f, self.expr()?));
                if !self.eat(",") {
                    self.expect("}")?;
                    break;
                }
            }
            let name = if self.eat_kw("as") { Some(self.ident()?) } else { None };
            return Ok(Stmt::Create(e, fs, name, span));
        }
        if self.eat_kw("touch") {
            return Ok(Stmt::Touch(self.expr()?, span));
        }
        if self.eat_kw("delete") {
            return Ok(Stmt::Delete(self.expr()?, span));
        }
        if self.eat_kw("let") {
            let n = self.ident()?;
            self.expect("=")?;
            return Ok(Stmt::Let(n, self.expr()?, span));
        }
        if self.eat_kw("if") {
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
        if self.eat_kw("signal") {
            let name = self.ident()?;
            self.expect("(")?;
            let arg = if self.is(")") { None } else { Some(self.expr()?) };
            self.expect(")")?;
            self.expect_kw("to")?;
            let who = self.ident()?;
            let to = if self.is("(") {
                self.expect("(")?;
                let a = self.expr()?;
                self.expect(")")?;
                SignalTo::Screen(who, a)
            } else {
                match who.as_str() {
                    "everyone" => SignalTo::Everyone,
                    "viewer" => SignalTo::Viewer,
                    _ => return self.err("a signal goes to everyone, viewer or a screen's audience: screen(arg)"),
                }
            };
            return Ok(Stmt::Signal(name, arg, to, span));
        }
        if self.eat_kw("sign") {
            if self.eat_kw("out") {
                return Ok(Stmt::SignOut(span));
            }
            self.expect_kw("in")?;
            return Ok(Stmt::SignIn(self.expr()?, span));
        }
        if self.eat_kw("set") {
            let target = self.postfix()?;
            let (obj, field) = match target {
                Expr::Field(o, f, _) => (*o, f),
                _ => return self.err("set needs a field: set row.field = value"),
            };
            self.expect("=")?;
            return Ok(Stmt::Set(obj, field, self.expr()?, span));
        }
        if self.eat_kw("for") {
            self.expect_kw("each")?;
            let v = self.ident()?;
            self.expect_kw("in")?;
            let q = match self.expr()? {
                Expr::Query(q) => *q,
                // every row of an entity
                Expr::Name(n, _) if is_entity_name(&n) => Query { source: Source::Entity(n), filter: None, order: vec![], limit: None, window_last: None, span },
                other => Query { source: Source::Expr(other), filter: None, order: vec![], limit: None, window_last: None, span },
            };
            let body = self.block()?;
            return Ok(Stmt::ForEach(v, q, body, span));
        }
        self.err(format!("expected a statement (require, create, set, touch, delete, let, if, go, sign in, sign out, for each), found {}", show(self.peek())))
    }

    // ---- a helper's statements

    /// `{ statement … }`
    fn hblock(&mut self) -> R<Vec<HStmt>> {
        self.expect("{")?;
        let mut out = vec![];
        while !self.eat("}") {
            out.push(self.hstmt()?);
        }
        Ok(out)
    }

    fn hstmt(&mut self) -> R<HStmt> {
        let span = self.span();
        if self.eat_kw("var") {
            let name = self.ident()?;
            self.expect("=")?;
            return Ok(HStmt::Var(name, self.expr()?, span));
        }
        if self.eat_kw("if") {
            let c = self.expr()?;
            let then = self.hblock()?;
            let otherwise = if self.eat_kw("else") {
                if self.is_kw("if") {
                    vec![self.hstmt()?]
                } else {
                    self.hblock()?
                }
            } else {
                vec![]
            };
            return Ok(HStmt::If(c, then, otherwise, span));
        }
        if self.eat_kw("while") {
            let c = self.expr()?;
            return Ok(HStmt::While(c, self.hblock()?, span));
        }
        if self.eat_kw("for") {
            let var = self.ident()?;
            self.expect_kw("in")?;
            let from = self.additive()?;
            self.expect("..")?;
            let to = self.additive()?;
            return Ok(HStmt::For(var, from, to, self.hblock()?, span));
        }
        if self.eat_kw("break") {
            return Ok(HStmt::Break(span));
        }
        if self.eat_kw("continue") {
            return Ok(HStmt::Continue(span));
        }
        if self.eat_kw("return") {
            let e = if self.is("}") { None } else { Some(self.expr()?) };
            return Ok(HStmt::Return(e, span));
        }
        if self.is_kw("out_byte") && matches!(self.peek_at(1), Tok::Punct("(")) {
            self.next();
            self.expect("(")?;
            let e = self.expr()?;
            self.expect(")")?;
            return Ok(HStmt::Out(e, span));
        }
        // `x = e`, `a[i] = e`
        let name = self.ident()?;
        if self.eat("[") {
            let i = self.expr()?;
            self.expect("]")?;
            self.expect("=")?;
            return Ok(HStmt::SetAt(name, i, self.expr()?, span));
        }
        if !self.eat("=") {
            return self.err(format!("a helper's statement: `var`, `if`, `while`, `for`, `break`, `continue`, `return`, `out_byte(…)`, or `{name} = …`"));
        }
        Ok(HStmt::Set(name, self.expr()?, span))
    }

    // ---- expressions, loosest first

    fn expr(&mut self) -> R<Expr> {
        let e = self.or()?;
        self.query_clauses(e)
    }

    /// `where …`, `order by …`, `limit …`, `window … last` after a source
    fn query_clauses(&mut self, e: Expr) -> R<Expr> {
        if !(self.is_kw("where") || self.is_kw("order") || self.is_kw("limit") || self.is_kw("window")) {
            return Ok(e);
        }
        let span = e.span();
        let source = match e {
            Expr::Name(n, _) if is_entity_name(&n) => Source::Entity(n),
            Expr::Query(q) if q.filter.is_none() && q.order.is_empty() && q.limit.is_none() && q.window_last.is_none() => q.source,
            other => Source::Expr(other),
        };
        let mut q = Query { source, filter: None, order: vec![], limit: None, window_last: None, span };
        if self.eat_kw("where") {
            q.filter = Some(self.or()?);
        }
        if self.eat_kw("order") {
            self.expect_kw("by")?;
            loop {
                let k = self.or()?;
                let desc = self.eat_kw("desc");
                q.order.push((k, desc));
                if !self.eat(",") {
                    break;
                }
            }
        }
        if self.eat_kw("limit") {
            q.limit = Some(self.additive()?);
        } else if self.eat_kw("window") {
            q.window_last = Some(self.additive()?);
            self.expect_kw("last")?;
        }
        Ok(Expr::Query(Box::new(q)))
    }

    fn or(&mut self) -> R<Expr> {
        let mut l = self.and()?;
        while self.is_kw("or") {
            let s = self.span();
            self.next();
            let r = self.and()?;
            l = Expr::Bin(BinOp::Or, Box::new(l), Box::new(r), s);
        }
        Ok(l)
    }
    fn and(&mut self) -> R<Expr> {
        let mut l = self.not()?;
        while self.is_kw("and") {
            let s = self.span();
            self.next();
            let r = self.not()?;
            l = Expr::Bin(BinOp::And, Box::new(l), Box::new(r), s);
        }
        Ok(l)
    }
    fn not(&mut self) -> R<Expr> {
        if self.is_kw("not") {
            let s = self.span();
            self.next();
            return Ok(Expr::Not(Box::new(self.not()?), s));
        }
        self.cmp()
    }
    fn cmp(&mut self) -> R<Expr> {
        let l = self.additive()?;
        let s = self.span();
        let op = match self.peek() {
            Tok::Punct("==") => BinOp::Eq,
            Tok::Punct("!=") => BinOp::Ne,
            Tok::Punct("<") => BinOp::Lt,
            Tok::Punct("<=") => BinOp::Le,
            Tok::Punct(">") => BinOp::Gt,
            Tok::Punct(">=") => BinOp::Ge,
            Tok::Ident(k) if k == "in" => BinOp::In,
            _ => return Ok(l),
        };
        self.next();
        let r = self.additive()?;
        Ok(Expr::Bin(op, Box::new(l), Box::new(r), s))
    }
    fn additive(&mut self) -> R<Expr> {
        let mut l = self.multiplicative()?;
        loop {
            let s = self.span();
            let op = match self.peek() {
                Tok::Punct("+") => BinOp::Add,
                Tok::Punct("-") => BinOp::Sub,
                Tok::Punct("+%") => BinOp::AddWrap,
                Tok::Punct("-%") => BinOp::SubWrap,
                Tok::Punct("|") => BinOp::BitOr,
                Tok::Punct("^") => BinOp::BitXor,
                _ => return Ok(l),
            };
            self.next();
            let r = self.multiplicative()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r), s);
        }
    }
    fn multiplicative(&mut self) -> R<Expr> {
        let mut l = self.postfix()?;
        loop {
            let s = self.span();
            // (as Go: shifts and `&` bind as multiplying does; `|` and `^` as adding)
            let op = match self.peek() {
                Tok::Punct("*") => BinOp::Mul,
                Tok::Punct("/") => BinOp::Div,
                Tok::Punct("%") => BinOp::Mod,
                Tok::Punct("*%") => BinOp::MulWrap,
                Tok::Punct("<<") => BinOp::Shl,
                Tok::Punct(">>") => BinOp::Shr,
                Tok::Punct("&") => BinOp::BitAnd,
                _ => return Ok(l),
            };
            self.next();
            let r = self.postfix()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r), s);
        }
    }
    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            let s = self.span();
            if self.eat(".") {
                let f = self.ident()?;
                e = Expr::Field(Box::new(e), f, s);
            } else if matches!(e, Expr::Name(..)) && self.eat("[") {
                // a helper's array, at an index
                let i = self.expr()?;
                self.expect("]")?;
                e = Expr::Call("__at".into(), vec![e, i], s);
            } else {
                return Ok(e);
            }
        }
    }
    fn primary(&mut self) -> R<Expr> {
        let s = self.span();
        match self.next() {
            Tok::Str(v) => Ok(Expr::Str(v, s)),
            Tok::Int(n) => Ok(Expr::Int(n, s)),
            Tok::Dur(n) => Ok(Expr::Dur(n, s)),
            // a negative number or duration (`-5`, `-30s`); any other value negated is `0 - x`
            Tok::Punct("-") => match self.next() {
                Tok::Int(n) => Ok(Expr::Int(-n, s)),
                Tok::Dur(n) => Ok(Expr::Dur(-n, s)),
                _ => {
                    self.i -= 1;
                    self.err("`-` before a value takes a number or a duration; write `0 - x` for anything else")
                }
            },
            Tok::Punct("(") => {
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Punct(".") => Ok(Expr::Dot(self.ident()?, s)),
            Tok::Punct("[") => {
                let mut items = vec![];
                while !self.eat("]") {
                    items.push(self.expr()?);
                    if !self.eat(",") {
                        self.expect("]")?;
                        break;
                    }
                }
                Ok(Expr::List(items, s))
            }
            Tok::Punct("{") => {
                let mut fs = vec![];
                while !self.eat("}") {
                    let n = self.ident()?;
                    self.expect(":")?;
                    fs.push((n, self.expr()?));
                    if !self.eat(",") {
                        self.expect("}")?;
                        break;
                    }
                }
                Ok(Expr::Record(fs, s))
            }
            Tok::Ident(n) => match n.as_str() {
                "true" => Ok(Expr::Bool(true, s)),
                "false" => Ok(Expr::Bool(false, s)),
                "none" => Ok(Expr::None(s)),
                "if" => {
                    let c = self.expr()?;
                    self.expect("{")?;
                    let a = self.expr()?;
                    self.expect("}")?;
                    self.expect_kw("else")?;
                    let b = if self.is_kw("if") {
                        self.primary()?
                    } else {
                        self.expect("{")?;
                        let b = self.expr()?;
                        self.expect("}")?;
                        b
                    };
                    Ok(Expr::If(Box::new(c), Box::new(a), Box::new(b), s))
                }
                _ if self.is("(") => {
                    self.next();
                    let mut args = vec![];
                    while !self.eat(")") {
                        let a = self.expr()?;
                        // `list(e for q)`, `count_distinct(e for q)`
                        if self.eat_kw("for") {
                            let q = match self.expr()? {
                                Expr::Query(q) => *q,
                                other => Query { source: Source::Expr(other), filter: None, order: vec![], limit: None, window_last: None, span: s },
                            };
                            self.expect(")")?;
                            return Ok(Expr::Comp(n, Box::new(a), Box::new(q), s));
                        }
                        args.push(a);
                        if !self.eat(",") {
                            self.expect(")")?;
                            break;
                        }
                    }
                    Ok(Expr::Call(n, args, s))
                }
                _ => Ok(Expr::Name(n, s)),
            },
            t => {
                self.i -= 1;
                self.err(format!("expected a value, found {}", show(&t)))
            }
        }
    }
}

fn show(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => format!("`{s}`"),
        Tok::Str(s) => format!("{s:?}"),
        Tok::Int(n) => n.to_string(),
        Tok::Dur(n) => format!("{n}ms"),
        Tok::Punct(p) => format!("`{p}`"),
        Tok::Template(_) => "a template".into(),
        Tok::Eof => "the end of the file".into(),
    }
}

fn parse_code(src: &str, span: Span) -> R<Parser> {
    let toks = Lexer { s: src.as_bytes(), i: 0, line: span.line, file: span.file }.tokens()?;
    Ok(Parser { toks, i: 0, file: span.file })
}

/// A template's text into nodes: `\` + newline joins lines; `{{ … }}` are holes.
fn parse_template(raw: &str, span: Span) -> R<Vec<TNode>> {
    // join lines ending in a backslash, dropping the next line's leading spaces
    let mut text = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find("\\\n") {
        text.push_str(&rest[..i]);
        rest = rest[i + 2..].trim_start_matches([' ', '\t']);
    }
    text.push_str(rest);

    // split into text and holes; a hole ends at `}}` outside a string
    enum Piece {
        Text(String),
        Hole(String, u32),
    }
    let mut pieces = vec![];
    let b = text.as_bytes();
    let (mut i, mut start, mut line) = (0, 0, span.line);
    while i < b.len() {
        if b[i] == b'\n' {
            line += 1;
        }
        if b[i..].starts_with(b"{{") {
            pieces.push(Piece::Text(text[start..i].to_string()));
            let hs = i + 2;
            let mut j = hs;
            let mut in_str = false;
            loop {
                if j >= b.len() {
                    return Err(Error { span: Span { file: span.file, line }, msg: "a hole ({{) isn't closed with }}".into() });
                }
                if in_str {
                    if b[j] == b'\\' {
                        j += 1;
                    } else if b[j] == b'"' {
                        in_str = false;
                    }
                } else if b[j] == b'"' {
                    in_str = true;
                } else if b[j..].starts_with(b"}}") {
                    break;
                }
                j += 1;
            }
            pieces.push(Piece::Hole(text[hs..j].trim().to_string(), line));
            i = j + 2;
            start = i;
            continue;
        }
        i += 1;
    }
    pieces.push(Piece::Text(text[start..].to_string()));

    // nest if/for blocks
    enum Frame {
        Root(Vec<TNode>),
        If(Expr, Vec<TNode>, Option<Vec<TNode>>),
        For(String, Expr, Vec<TNode>, bool),
    }
    let mut stack = vec![Frame::Root(vec![])];
    // `{{let x = e}}`: names for the rest of the block they're in (and blocks inside it), put in
    // place where they're used: the passes after parsing never see them. A `for`'s variable
    // hides a name of the same name (None).
    let mut lets: Vec<Vec<(String, Option<Expr>)>> = vec![vec![]];
    fn push(stack: &mut [Frame], n: TNode) {
        match stack.last_mut().unwrap() {
            Frame::Root(v) | Frame::For(_, _, v, _) => v.push(n),
            Frame::If(_, a, None) => a.push(n),
            Frame::If(_, _, Some(b)) => b.push(n),
        }
    }
    for p in pieces {
        match p {
            Piece::Text(t) => {
                if !t.is_empty() {
                    push(&mut stack, TNode::Text(t));
                }
            }
            Piece::Hole(code, line) => {
                let sp = Span { file: span.file, line };
                let herr = |msg: String| Error { span: sp, msg };
                if let Some(c) = code.strip_prefix("let ") {
                    let mut p = parse_code(c, sp)?;
                    let name = p.ident()?;
                    p.expect("=")?;
                    let e = with_lets(&p.expr()?, &lets);
                    if p.peek() != &Tok::Eof {
                        return p.err("unexpected text after the value in {{let}}");
                    }
                    lets.last_mut().unwrap().push((name, Some(e)));
                } else if let Some(c) = code.strip_prefix("if ") {
                    let mut p = parse_code(c, sp)?;
                    let e = with_lets(&p.expr()?, &lets);
                    stack.push(Frame::If(e, vec![], None));
                    lets.push(vec![]);
                } else if code == "else" {
                    match stack.last_mut() {
                        Some(Frame::If(_, _, b)) if b.is_none() => *b = Some(vec![]),
                        _ => return Err(herr("{{else}} outside an {{if}}".into())),
                    }
                    // the then-branch's names end with it
                    lets.last_mut().unwrap().clear();
                } else if code == "end" {
                    lets.pop();
                    if lets.is_empty() {
                        return Err(herr("{{end}} with nothing open".into()));
                    }
                    let f = stack.pop().unwrap();
                    let n = match f {
                        Frame::If(c, a, b) => TNode::If(c, a, b.unwrap_or_default()),
                        Frame::For(v, e, body, live) => TNode::For(v, e, None, body, live),
                        Frame::Root(_) => return Err(herr("{{end}} with nothing open".into())),
                    };
                    if stack.is_empty() {
                        return Err(herr("{{end}} with nothing open".into()));
                    }
                    push(&mut stack, n);
                } else if let Some(c) = code.strip_prefix("for ") {
                    let mut p = parse_code(c, sp)?;
                    let v = p.ident()?;
                    p.expect_kw("in")?;
                    let e = with_lets(&p.expr()?, &lets);
                    let live = p.eat_kw("live");
                    if p.peek() != &Tok::Eof {
                        return p.err("unexpected text after the list in {{for}}");
                    }
                    if live {
                        // the element the list is in gets its live id (what the list's changes
                        // target): `live_<view>` (a page has one of each list; its stream says
                        // which instance)
                        let view = match &e {
                            Expr::Call(f, a, _) if a.len() == 1 => f.clone(),
                            _ => return Err(herr("a live list is a view of one row: `{{for x in view(row) live}}`".into())),
                        };
                        let last = match stack.last_mut().unwrap() {
                            Frame::Root(v) | Frame::For(_, _, v, _) => v,
                            Frame::If(_, a, None) => a,
                            Frame::If(_, _, Some(b)) => b,
                        };
                        let Some(TNode::Text(t)) = last.last_mut() else {
                            return Err(herr("a live list goes right inside an element: `<div …>{{for x in view(row) live}}`".into()));
                        };
                        let open = t.rfind('<').filter(|&i| t.ends_with('>') && !t[i..].starts_with("</") && !t[i..].contains("id="));
                        let Some(_) = open else {
                            return Err(herr("a live list goes right inside an element (with no id of its own): `<div …>{{for x in view(row) live}}`".into()));
                        };
                        t.pop();
                        t.push_str(&format!(" id=\"live_{view}\">"));
                    }
                    lets.push(vec![(v.clone(), None)]);
                    stack.push(Frame::For(v, e, vec![], live));
                } else {
                    let mut p = parse_code(&code, sp)?;
                    let e = with_lets(&p.expr()?, &lets);
                    if p.peek() != &Tok::Eof {
                        return p.err(format!("unexpected {} in a hole", show(p.peek())));
                    }
                    push(&mut stack, TNode::Hole(e));
                }
            }
        }
    }
    match stack.pop() {
        Some(Frame::Root(v)) if stack.is_empty() => Ok(v),
        _ => Err(Error { span, msg: "an {{if}} or {{for}} isn't closed with {{end}}".into() }),
    }
}

/// `e` with the template's `{{let}}` names in scope put in place (the innermost first).
fn with_lets(e: &Expr, lets: &[Vec<(String, Option<Expr>)>]) -> Expr {
    let find = |n: &str| lets.iter().rev().flat_map(|f| f.iter().rev()).find(|(k, _)| k == n).map(|(_, v)| v.clone());
    subst_names(e, &|n| find(n).flatten())
}

/// `e` with each name `f` gives a value for replaced by it (also inside queries).
fn subst_names(e: &Expr, f: &dyn Fn(&str) -> Option<Expr>) -> Expr {
    let go = |x: &Expr| subst_names(x, f);
    let q = |q: &Query| Query {
        source: match &q.source {
            Source::Expr(x) => Source::Expr(go(x)),
            s => s.clone(),
        },
        filter: q.filter.as_ref().map(go),
        order: q.order.iter().map(|(k, d)| (go(k), *d)).collect(),
        limit: q.limit.as_ref().map(go),
        window_last: q.window_last.as_ref().map(go),
        span: q.span,
    };
    match e {
        Expr::Name(n, _) => f(n).unwrap_or_else(|| e.clone()),
        Expr::Field(o, n, s) => Expr::Field(Box::new(go(o)), n.clone(), *s),
        Expr::Call(n, a, s) => Expr::Call(n.clone(), a.iter().map(go).collect(), *s),
        Expr::Bin(op, l, r, s) => Expr::Bin(op.clone(), Box::new(go(l)), Box::new(go(r)), *s),
        Expr::Not(x, s) => Expr::Not(Box::new(go(x)), *s),
        Expr::If(c, a, b, s) => Expr::If(Box::new(go(c)), Box::new(go(a)), Box::new(go(b)), *s),
        Expr::List(i, s) => Expr::List(i.iter().map(go).collect(), *s),
        Expr::Record(fs, s) => Expr::Record(fs.iter().map(|(n, v)| (n.clone(), go(v))).collect(), *s),
        Expr::Query(x) => Expr::Query(Box::new(q(x))),
        Expr::Comp(n, x, qy, s) => Expr::Comp(n.clone(), Box::new(go(x)), Box::new(q(qy)), *s),
        other => other.clone(),
    }
}

/// Parses one file's declarations.
pub fn parse_file(src: &str, file: u32) -> R<Vec<Decl>> {
    let mut p = parse_code(src, Span { file, line: 1 })?;
    let mut out = vec![];
    while p.peek() != &Tok::Eof {
        let d = p.decl()?;
        // a helper is also declared as an external function (its signature, its symbol): it's
        // called, placed and checked as one
        if let Decl::Helper { name, params, ret, span, .. } = &d {
            out.push(Decl::Extern { name: name.clone(), params: params.clone(), ret: ret.clone(), effect: false, symbol: format!("burin_fn_{name}"), span: *span });
        }
        out.push(d);
    }
    let _ = p.file;
    Ok(out)
}

/// Parses one expression (the tests').
#[cfg(test)]
pub fn parse_expr(src: &str) -> R<Expr> {
    let mut p = parse_code(src, Span::default())?;
    let e = p.expr()?;
    if p.peek() != &Tok::Eof {
        return p.err("more after the expression");
    }
    Ok(e)
}

/// Parses every .burin file in an app's directory, in name order.
pub fn parse_app(dir: &std::path::Path) -> Result<App, (Vec<String>, Error)> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| (vec![], Error { span: Span::default(), msg: format!("{}: {e}", dir.display()) }))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "burin").unwrap_or(false))
        .collect();
    paths.sort();
    let mut files: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let mut decls = vec![];
    for (i, p) in paths.iter().enumerate() {
        let src = std::fs::read_to_string(p).map_err(|e| (files.clone(), Error { span: Span { file: i as u32, line: 0 }, msg: e.to_string() }))?;
        decls.extend(parse_file(&src, i as u32).map_err(|e| (files.clone(), e))?);
    }
    files.push(PRELUDE_NAME.into());
    decls.extend(prelude(files.len() as u32 - 1).map_err(|e| (files.clone(), e))?);
    Ok(App { files, decls })
}

/// The standard library every app has (lang/std.burin, built into the compiler).
pub const PRELUDE: &str = include_str!("../../../lang/std.burin");
pub const PRELUDE_NAME: &str = "lang/std.burin";

/// The standard library's declarations, as file `file`.
pub fn prelude(file: u32) -> R<Vec<Decl>> {
    parse_file(PRELUDE, file)
}
