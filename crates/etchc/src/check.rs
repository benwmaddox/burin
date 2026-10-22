//! The checker: names, types, and the rules that keep apps efficient (DESIGN.md, "The language
//! leans toward efficient apps"): holes typed for where they are in the HTML, and every list a
//! page shows bounded.

use crate::ast::*;
use std::collections::HashMap;

pub struct Error {
    pub span: Span,
    pub msg: String,
}

type R<T> = Result<T, Error>;

fn err<T>(span: Span, msg: impl Into<String>) -> R<T> {
    Err(Error { span, msg: msg.into() })
}

/// Everything declared, by name.
pub struct Model<'a> {
    pub app: &'a App,
    pub configs: HashMap<String, Type>,
    pub entities: HashMap<String, &'a Entity>,
    pub viewer: Option<&'a Decl>,
    pub fns: HashMap<String, &'a Decl>,
    pub views: HashMap<String, &'a Decl>,
    pub consts: HashMap<String, &'a Decl>,
    pub fragments: HashMap<String, &'a Decl>,
    pub screens: Vec<&'a Decl>,
    pub actions: Vec<&'a Decl>,
}

/// The fields every entity has.
pub const IMPLICIT: [(&str, Type); 4] = [("id", Type::Int), ("seq", Type::Int), ("created", Type::Time), ("updated", Type::Time)];

impl<'a> Model<'a> {
    pub fn new(app: &'a App) -> R<Model<'a>> {
        let mut m = Model {
            app,
            configs: HashMap::new(),
            entities: HashMap::new(),
            viewer: None,
            fns: HashMap::new(),
            views: HashMap::new(),
            consts: HashMap::new(),
            fragments: HashMap::new(),
            screens: vec![],
            actions: vec![],
        };
        for d in &app.decls {
            let dup = |span: Span, n: &str| err::<()>(span, format!("`{n}` is declared twice"));
            match d {
                Decl::Config { name, ty, span } => {
                    if m.configs.insert(name.clone(), ty.clone()).is_some() {
                        dup(*span, name)?
                    }
                }
                Decl::Entity(e) => {
                    if m.entities.insert(e.name.clone(), e).is_some() {
                        dup(e.span, &e.name)?
                    }
                }
                Decl::Viewer { span, .. } => {
                    if m.viewer.replace(d).is_some() {
                        err(*span, "there is one viewer declaration")?
                    }
                }
                Decl::Fn { name, span, .. } => {
                    if m.fns.insert(name.clone(), d).is_some() {
                        dup(*span, name)?
                    }
                }
                Decl::View { name, span, .. } => {
                    if m.views.insert(name.clone(), d).is_some() {
                        dup(*span, name)?
                    }
                }
                Decl::Const { name, span, .. } => {
                    if m.consts.insert(name.clone(), d).is_some() {
                        dup(*span, name)?
                    }
                }
                Decl::Fragment { name, span, .. } => {
                    if m.fragments.insert(name.clone(), d).is_some() {
                        dup(*span, name)?
                    }
                }
                Decl::Screen { .. } => m.screens.push(d),
                Decl::Action { .. } => m.actions.push(d),
            }
        }
        Ok(m)
    }

    pub fn viewer_entity(&self) -> Option<&str> {
        match self.viewer {
            Some(Decl::Viewer { entity, .. }) => Some(entity),
            _ => None,
        }
    }

    pub fn field_type(&self, entity: &str, field: &str) -> Option<Type> {
        if let Some((_, t)) = IMPLICIT.iter().find(|(n, _)| *n == field) {
            return Some(t.clone());
        }
        self.entities.get(entity)?.fields.iter().find(|f| f.name == field).map(|f| f.ty.clone())
    }
}

/// Names in scope, innermost last.
#[derive(Clone, Default)]
pub struct Scope {
    vars: Vec<(String, Type)>,
}

impl Scope {
    pub fn with(&self, n: &str, t: Type) -> Scope {
        let mut s = self.clone();
        s.vars.push((n.to_string(), t));
        s
    }
    pub fn get(&self, n: &str) -> Option<&Type> {
        self.vars.iter().rev().find(|(k, _)| k == n).map(|(_, t)| t)
    }
    pub fn of(params: &[Param]) -> Scope {
        Scope { vars: params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect() }
    }
}

fn compatible(want: &Type, got: &Type) -> bool {
    match (want, got) {
        (a, b) if a == b => true,
        (Type::Opt(_), Type::None) => true,
        (Type::Opt(a), b) => compatible(a, b),
        (a, Type::Opt(b)) => compatible(a, b), // an optional read where a value is expected: checked at run time (none is "")
        (Type::List(a), Type::List(b)) => compatible(a, b),
        _ => false,
    }
}

impl<'a> Model<'a> {
    /// The rows a query gives: the entity of its source.
    pub fn query_entity(&self, q: &Query, scope: &Scope) -> R<String> {
        match &q.source {
            Source::Entity(e) => {
                if !self.entities.contains_key(e) {
                    return err(q.span, format!("no entity `{e}`"));
                }
                Ok(e.clone())
            }
            Source::Expr(e) => match self.type_of(e, scope)? {
                Type::List(t) => match *t {
                    Type::Ref(e) => Ok(e),
                    t => err(q.span, format!("a query's source must be rows, not {}", t.show())),
                },
                t => err(q.span, format!("a query's source must be rows, not {}", t.show())),
            },
        }
    }

    pub fn check_query(&self, q: &Query, scope: &Scope) -> R<Type> {
        let e = self.query_entity(q, scope)?;
        let inner = scope.with("it", Type::Ref(e.clone()));
        if let Some(f) = &q.filter {
            let t = self.type_of(f, &inner)?;
            if t != Type::Bool {
                return err(f.span(), format!("a query's `where` must be a Bool, not {}", t.show()));
            }
        }
        for (k, _) in &q.order {
            self.type_of(k, &inner)?;
        }
        for n in q.limit.iter().chain(q.window_last.iter()) {
            if self.type_of(n, scope)? != Type::Int {
                return err(n.span(), "a limit or window is an Int");
            }
        }
        if q.window_last.is_some() && q.order.is_empty() {
            return err(q.span, "`window N last` needs an order");
        }
        Ok(Type::List(Box::new(Type::Ref(e))))
    }

    pub fn type_of(&self, e: &Expr, scope: &Scope) -> R<Type> {
        Ok(match e {
            Expr::Str(..) => Type::Text,
            Expr::Int(..) => Type::Int,
            Expr::Bool(..) => Type::Bool,
            Expr::Dur(..) => Type::Duration,
            Expr::None(_) => Type::None,
            Expr::Name(n, s) => {
                if let Some(t) = scope.get(n) {
                    return Ok(t.clone());
                }
                match n.as_str() {
                    "viewer" => Type::Ref(self.viewer_entity().map(String::from).ok_or_else(|| Error { span: *s, msg: "`viewer` without a viewer declaration".into() })?),
                    "now" => Type::Time,
                    "config" => return err(*s, "`config` takes a field: config.name"),
                    _ if self.entities.contains_key(n) => Type::List(Box::new(Type::Ref(n.clone()))),
                    _ => match self.consts.get(n) {
                        Some(Decl::Const { value, .. }) => self.type_of(value, &Scope::default())?,
                        _ => return err(*s, format!("`{n}` isn't declared here")),
                    },
                }
            }
            Expr::Dot(f, s) => match scope.get("it") {
                Some(Type::Ref(ent)) => self.field_type(ent, f).ok_or_else(|| Error { span: *s, msg: format!("{ent} has no field `{f}`") })?,
                _ => return err(*s, format!("`.{f}` outside a query")),
            },
            Expr::Field(o, f, s) => {
                if let Expr::Name(n, _) = &**o {
                    if n == "config" && scope.get("config").is_none() {
                        return self.configs.get(f).cloned().ok_or_else(|| Error { span: *s, msg: format!("no config `{f}`") });
                    }
                }
                match self.type_of(o, scope)?.strip_opt() {
                    Type::Ref(ent) => self.field_type(ent, f).ok_or_else(|| Error { span: *s, msg: format!("{ent} has no field `{f}`") })?,
                    Type::Record(fs) => fs.iter().find(|(n, _)| n == f).map(|(_, t)| t.clone()).ok_or_else(|| Error { span: *s, msg: format!("no field `{f}`") })?,
                    t => return err(*s, format!("{} has no fields", t.show())),
                }
            }
            Expr::Not(x, s) => {
                if self.type_of(x, scope)? != Type::Bool {
                    return err(*s, "`not` takes a Bool");
                }
                Type::Bool
            }
            Expr::Bin(op, l, r, s) => {
                let (a, b) = (self.type_of(l, scope)?, self.type_of(r, scope)?);
                match op {
                    BinOp::Or | BinOp::And => {
                        if a != Type::Bool || b != Type::Bool {
                            return err(*s, "`and` and `or` take Bools");
                        }
                        Type::Bool
                    }
                    BinOp::Eq | BinOp::Ne => {
                        if !(compatible(&a, &b) || compatible(&b, &a)) {
                            return err(*s, format!("can't compare {} with {}", a.show(), b.show()));
                        }
                        Type::Bool
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        let ok = matches!((a.strip_opt(), b.strip_opt()), (Type::Int, Type::Int) | (Type::Time, Type::Time) | (Type::Text, Type::Text));
                        if !ok {
                            return err(*s, format!("can't order {} and {}", a.show(), b.show()));
                        }
                        Type::Bool
                    }
                    BinOp::Add => match (a.strip_opt(), b.strip_opt()) {
                        (Type::Text, Type::Text) => Type::Text,
                        (Type::Int, Type::Int) => Type::Int,
                        (Type::Time, Type::Duration) => Type::Time,
                        _ => return err(*s, format!("can't add {} and {}", a.show(), b.show())),
                    },
                    BinOp::Sub => match (a.strip_opt(), b.strip_opt()) {
                        (Type::Int, Type::Int) => Type::Int,
                        (Type::Time, Type::Duration) => Type::Time,
                        _ => return err(*s, format!("can't subtract {} from {}", b.show(), a.show())),
                    },
                }
            }
            Expr::If(c, a, b, s) => {
                if self.type_of(c, scope)? != Type::Bool {
                    return err(*s, "`if` takes a Bool");
                }
                let (x, y) = (self.type_of(a, scope)?, self.type_of(b, scope)?);
                if !compatible(&x, &y) && !compatible(&y, &x) {
                    return err(*s, format!("the branches differ: {} and {}", x.show(), y.show()));
                }
                x
            }
            Expr::List(items, _) => {
                let t = match items.first() {
                    Some(i) => self.type_of(i, scope)?,
                    None => Type::None,
                };
                for i in items {
                    let u = self.type_of(i, scope)?;
                    if u != t {
                        return err(i.span(), format!("a list's items differ: {} and {}", t.show(), u.show()));
                    }
                }
                Type::List(Box::new(t))
            }
            Expr::Record(fs, _) => Type::Record(fs.iter().map(|(n, e)| Ok((n.clone(), self.type_of(e, scope)?))).collect::<R<_>>()?),
            Expr::Query(q) => self.check_query(q, scope)?,
            Expr::Comp(f, x, q, s) => {
                let ent = self.query_entity(q, scope)?;
                self.check_query(q, scope)?;
                let t = self.type_of(x, &scope.with("it", Type::Ref(ent)))?;
                match f.as_str() {
                    "list" => Type::List(Box::new(t)),
                    "count_distinct" => Type::Int,
                    _ => return err(*s, format!("`{f}(… for …)` isn't a function; list and count_distinct are")),
                }
            }
            Expr::Call(f, args, s) => return self.call_type(f, args, scope, *s),
        })
    }

    fn call_type(&self, f: &str, args: &[Expr], scope: &Scope, s: Span) -> R<Type> {
        let arity = |n: usize| if args.len() == n { Ok(()) } else { err(s, format!("`{f}` takes {n} argument(s)")) };
        let arg = |i: usize, want: Type| -> R<()> {
            let t = self.type_of(&args[i], scope)?;
            if compatible(&want, &t) {
                Ok(())
            } else {
                err(args[i].span(), format!("`{f}` takes {} here, not {}", want.show(), t.show()))
            }
        };
        let rows = |i: usize| -> R<Type> {
            match self.type_of(&args[i], scope)? {
                Type::List(t) => Ok(*t),
                t => err(args[i].span(), format!("`{f}` takes rows, not {}", t.show())),
            }
        };
        match f {
            "asset" | "lower" | "first_word" | "initials" | "quote" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Text)
            }
            "json" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Json)
            }
            "sign" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Text)
            }
            "text" => {
                arity(1)?;
                arg(0, Type::Int)?;
                Ok(Type::Text)
            }
            "iso" | "date" | "clock" => {
                arity(1)?;
                arg(0, Type::Time)?;
                Ok(Type::Text)
            }
            "autolink" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Html)
            }
            "replace" => {
                arity(3)?;
                for i in 0..3 {
                    arg(i, Type::Text)?;
                }
                Ok(Type::Text)
            }
            "sentence" => {
                arity(1)?;
                arg(0, Type::List(Box::new(Type::Text)))?;
                Ok(Type::Text)
            }
            "present" => {
                arity(1)?;
                arg(0, Type::Text)?;
                Ok(Type::Bool)
            }
            "max" => {
                arity(2)?;
                arg(0, Type::Int)?;
                arg(1, Type::Int)?;
                Ok(Type::Int)
            }
            "token" => {
                arity(1)?;
                arg(0, Type::Int)?;
                Ok(Type::Text)
            }
            "include" => {
                arity(1)?;
                match &args[0] {
                    Expr::Str(..) => Ok(Type::Html),
                    _ => err(s, "include takes a literal path"),
                }
            }
            "count" => {
                arity(1)?;
                rows(0)?;
                Ok(Type::Int)
            }
            "exists" => {
                arity(1)?;
                rows(0)?;
                Ok(Type::Bool)
            }
            "first" => {
                arity(1)?;
                rows(0)
            }
            _ => {
                let (params, ret) = if let Some(Decl::Fn { params, ret, .. }) = self.fns.get(f) {
                    (params, ret.clone())
                } else if let Some(Decl::View { params, query, .. }) = self.views.get(f) {
                    let ent = self.query_entity(query, &Scope::of(params))?;
                    (params, Type::List(Box::new(Type::Ref(ent))))
                } else if let Some(Decl::Fragment { params, .. }) = self.fragments.get(f) {
                    (params, Type::Html)
                } else {
                    return err(s, format!("no function, view or fragment `{f}`"));
                };
                arity(params.len())?;
                for (i, p) in params.iter().enumerate() {
                    arg(i, p.ty.clone())?;
                }
                Ok(ret)
            }
        }
    }
}

// ---- holes, by where they are in the HTML

#[derive(Clone, Debug, PartialEq)]
enum Html {
    Text,
    /// inside a tag, before an attribute's value
    Tag,
    /// inside an attribute's value (its name, its quote)
    Val(String, u8),
    /// inside <script> … </script>
    Script,
}

fn scan(mut st: Html, text: &str) -> Html {
    let b = text.as_bytes();
    let mut i = 0;
    let mut attr = String::new();
    while i < b.len() {
        let c = b[i];
        st = match st {
            Html::Text => {
                if text[i..].starts_with("<script") {
                    // the tag's attributes, then the script
                    match text[i..].find('>') {
                        Some(j) => {
                            i += j + 1;
                            Html::Script
                        }
                        None => Html::Tag,
                    }
                } else if c == b'<' && b.get(i + 1).map(|d| d.is_ascii_alphabetic() || *d == b'/' || *d == b'!').unwrap_or(false) {
                    attr.clear();
                    Html::Tag
                } else {
                    Html::Text
                }
            }
            Html::Script => {
                if text[i..].starts_with("</script>") {
                    i += "</script>".len() - 1;
                    Html::Text
                } else {
                    Html::Script
                }
            }
            Html::Tag => match c {
                b'>' => Html::Text,
                b'"' | b'\'' => Html::Val(attr.clone(), c),
                b'=' => Html::Tag,
                c if c.is_ascii_whitespace() => {
                    attr.clear();
                    Html::Tag
                }
                c => {
                    attr.push(c as char);
                    Html::Tag
                }
            },
            Html::Val(a, q) => {
                if c == q {
                    attr.clear();
                    Html::Tag
                } else {
                    Html::Val(a, q)
                }
            }
        };
        if !matches!(st, Html::Script) || i < b.len() {
            i += 1;
        }
    }
    st
}

impl<'a> Model<'a> {
    fn check_hole(&self, e: &Expr, at: &Html, scope: &Scope) -> R<()> {
        let t = self.type_of(e, scope)?;
        let t = t.strip_opt().clone();
        let ok = match at {
            Html::Val(a, _) if a.to_ascii_lowercase().starts_with("on") => t == Type::Json,
            Html::Val(..) => matches!(t, Type::Text | Type::Int),
            Html::Text => matches!(t, Type::Text | Type::Int | Type::Html),
            Html::Tag => false,
            Html::Script => false,
        };
        if ok {
            return Ok(());
        }
        let where_ = match at {
            Html::Val(a, _) => format!("the `{a}` attribute"),
            Html::Text => "text".into(),
            Html::Tag => "a tag (outside an attribute's value)".into(),
            Html::Script => "a script".into(),
        };
        let hint = match at {
            Html::Val(a, _) if a.to_ascii_lowercase().starts_with("on") => ": a hole in an event handler takes only json(…)",
            Html::Tag | Html::Script => ": holes go in text or in quoted attribute values",
            _ if t == Type::Html => ": Html goes only in text",
            _ => "",
        };
        err(e.span(), format!("a {} can't go in {where_}{hint}", t.show()))
    }

    fn check_nodes(&self, nodes: &[TNode], mut at: Html, scope: &Scope, bounded_needed: bool) -> R<Html> {
        for n in nodes {
            match n {
                TNode::Text(t) => at = scan(at, t),
                TNode::Hole(e) => self.check_hole(e, &at, scope)?,
                TNode::If(c, a, b) => {
                    if self.type_of(c, scope)?.strip_opt() != &Type::Bool {
                        return err(c.span(), "{{if}} takes a Bool");
                    }
                    let x = self.check_nodes(a, at.clone(), scope, bounded_needed)?;
                    let y = self.check_nodes(b, at.clone(), scope, bounded_needed)?;
                    if x != y {
                        return err(c.span(), "the two sides of this {{if}} end in different places in the HTML");
                    }
                    at = x;
                }
                TNode::For(v, list, _, body) => {
                    let t = match self.type_of(list, scope)? {
                        Type::List(t) => *t,
                        t => return err(list.span(), format!("{{{{for}}}} takes a list, not {}", t.show())),
                    };
                    if bounded_needed && self.bound_of(list, scope).is_none() {
                        return err(list.span(), "every list a page shows is bounded: give the view `limit N` or `window N last`");
                    }
                    let end = self.check_nodes(body, at.clone(), &scope.with(v, t), bounded_needed)?;
                    if end != at {
                        return err(list.span(), "a {{for}}'s body must end where it starts in the HTML");
                    }
                }
            }
        }
        Ok(at)
    }

    /// A list's most items, if the spec bounds it.
    pub fn bound_of(&self, list: &Expr, scope: &Scope) -> Option<i64> {
        let lit = |e: &Expr| match e {
            Expr::Int(n, _) => Some(*n),
            Expr::Call(f, a, _) if f == "max" => {
                // max(0, N - …) is at most N when the subtracted part isn't negative
                match (a.first(), a.get(1)) {
                    (Some(Expr::Int(_, _)), Some(Expr::Bin(BinOp::Sub, l, _, _))) => match &**l {
                        Expr::Int(n, _) => Some(*n),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        let q_bound = |q: &Query| -> Option<i64> {
            q.window_last.as_ref().and_then(lit).or_else(|| q.limit.as_ref().and_then(lit))
        };
        match list {
            Expr::List(items, _) => Some(items.len() as i64),
            Expr::Name(n, _) => match self.consts.get(n) {
                Some(Decl::Const { value: Expr::List(items, _), .. }) => Some(items.len() as i64),
                _ => None,
            },
            Expr::Query(q) => {
                let own = q_bound(q);
                let inner = match &q.source {
                    Source::Expr(e) => self.bound_of(e, scope),
                    Source::Entity(_) => None,
                };
                match (own, inner) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            }
            Expr::Call(f, _, _) => match self.views.get(f) {
                Some(Decl::View { query, params, .. }) => self.bound_of(&Expr::Query(Box::new(query.clone())), &Scope::of(params)),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Checks the whole app; the first error stops it.
pub fn check(m: &Model) -> R<()> {
    for e in m.entities.values() {
        for f in &e.fields {
            if let Type::Ref(t) = f.ty.strip_opt() {
                if !m.entities.contains_key(t) {
                    return err(e.span, format!("{}.{}: no entity `{t}`", e.name, f.name));
                }
            }
        }
    }
    if let Some(Decl::Viewer { entity, via_entity, via_field, to_field, span, .. }) = m.viewer {
        if m.field_type(via_entity, via_field) != Some(Type::Text) {
            return err(*span, format!("{via_entity}.{via_field} must be a Text field"));
        }
        if m.field_type(via_entity, to_field) != Some(Type::Ref(entity.clone())) {
            return err(*span, format!("{via_entity}.{to_field} must refer to {entity}"));
        }
    }
    for d in &m.app.decls {
        match d {
            Decl::Fn { params, ret, body, span, .. } => {
                let t = m.type_of(body, &Scope::of(params))?;
                if !compatible(ret, &t) {
                    return err(*span, format!("returns {}, not {}", t.show(), ret.show()));
                }
            }
            Decl::View { params, query, .. } => {
                m.check_query(query, &Scope::of(params))?;
            }
            Decl::Const { value, .. } => {
                m.type_of(value, &Scope::default())?;
            }
            Decl::Fragment { params, body, .. } => {
                let end = m.check_nodes(&body.nodes, Html::Text, &Scope::of(params), true)?;
                if end != Html::Text {
                    return err(body.span, "a fragment must end in text, outside any tag");
                }
            }
            Decl::Screen { params, requires, cookies, body, route, span, .. } => {
                let scope = Scope::of(params);
                for p in params {
                    if !route.contains(&format!("{{{}}}", p.name)) {
                        return err(*span, format!("the route doesn't name the parameter `{}`", p.name));
                    }
                }
                for r in requires {
                    if m.type_of(&r.cond, &scope)? != Type::Bool {
                        return err(r.cond.span(), "require takes a Bool");
                    }
                }
                for (_, e) in cookies {
                    if !matches!(m.type_of(e, &scope)?.strip_opt(), Type::Text) {
                        return err(e.span(), "a cookie's value is Text");
                    }
                }
                m.check_nodes(&body.nodes, Html::Text, &scope, true)?;
            }
            Decl::Action { params, body, .. } => check_stmts(m, body, &Scope::of(params), &mut vec![])?,
            _ => {}
        }
    }
    Ok(())
}

/// `.f == x` among a filter's conjuncts: the field f compared with the name x.
fn eq_fields(e: &Expr, out: &mut Vec<(String, String)>) {
    match e {
        Expr::Bin(BinOp::And, l, r, _) => {
            eq_fields(l, out);
            eq_fields(r, out);
        }
        Expr::Bin(BinOp::Eq, l, r, _) => match (&**l, &**r) {
            (Expr::Dot(f, _), Expr::Name(x, _)) | (Expr::Name(x, _), Expr::Dot(f, _)) => out.push((f.clone(), x.clone())),
            _ => {}
        },
        _ => {}
    }
}

/// `cleared`: (entity, reference field, name) for each earlier `for each y in F where .f == x
/// { delete y }` (or `{ set y.f = none }`): no F row refers to x through f any more.
fn check_stmts(m: &Model, body: &[Stmt], scope: &Scope, cleared: &mut Vec<(String, String, String)>) -> R<()> {
    for s in body {
        match s {
            Stmt::Require(r) => {
                if m.type_of(&r.cond, scope)? != Type::Bool {
                    return err(r.cond.span(), "require takes a Bool");
                }
            }
            Stmt::Create(e, fs, span) => {
                if !m.entities.contains_key(e) {
                    return err(*span, format!("no entity `{e}`"));
                }
                for (f, v) in fs {
                    let want = m.field_type(e, f).ok_or_else(|| Error { span: *span, msg: format!("{e} has no field `{f}`") })?;
                    let got = m.type_of(v, scope)?;
                    if !compatible(&want, &got) {
                        return err(v.span(), format!("{e}.{f} is {}, not {}", want.show(), got.show()));
                    }
                }
            }
            Stmt::Touch(e, span) => {
                if !matches!(m.type_of(e, scope)?, Type::Ref(_)) {
                    return err(*span, "touch takes a row");
                }
            }
            Stmt::Set(o, f, v, span) => {
                let ent = match m.type_of(o, scope)? {
                    Type::Ref(e) => e,
                    t => return err(*span, format!("set takes a row's field, not {}'s", t.show())),
                };
                let want = m.field_type(&ent, f).ok_or_else(|| Error { span: *span, msg: format!("{ent} has no field `{f}`") })?;
                let got = m.type_of(v, scope)?;
                if !compatible(&want, &got) {
                    return err(v.span(), format!("{ent}.{f} is {}, not {}", want.show(), got.show()));
                }
            }
            Stmt::ForEach(v, q, inner, _) => {
                let ent = m.query_entity(q, scope)?;
                m.check_query(q, scope)?;
                check_stmts(m, inner, &scope.with(v, Type::Ref(ent.clone())), cleared)?;
                let mut eqs = vec![];
                if let Some(f) = &q.filter {
                    eq_fields(f, &mut eqs);
                }
                for (f, x) in eqs {
                    let clears = inner.iter().any(|s| match s {
                        Stmt::Delete(Expr::Name(y, _), _) => y == v,
                        Stmt::Set(Expr::Name(y, _), g, Expr::None(_), _) => y == v && *g == f,
                        _ => false,
                    });
                    if clears {
                        cleared.push((ent.clone(), f, x));
                    }
                }
            }
            Stmt::Delete(e, span) => {
                let ent = match m.type_of(e, scope)? {
                    Type::Ref(t) => t,
                    t => return err(*span, format!("delete takes a row, not {}", t.show())),
                };
                let x = match e {
                    Expr::Name(x, _) => x.clone(),
                    _ => return err(*span, "delete takes a parameter or a loop's row by its name"),
                };
                // no row may be left referring to it
                let mut ents: Vec<&&Entity> = m.entities.values().collect();
                ents.sort_by(|a, b| a.name.cmp(&b.name));
                for other in ents {
                    for f in &other.fields {
                        if matches!(f.ty.strip_opt(), Type::Ref(t) if *t == ent) && !cleared.iter().any(|(e2, f2, x2)| *e2 == other.name && *f2 == f.name && *x2 == x) {
                            return err(
                                *span,
                                format!(
                                    "deleting {x} would leave {}.{} referring to it: first `for each y in {} where .{} == {x} {{ delete y }}`{}",
                                    other.name,
                                    f.name,
                                    other.name,
                                    f.name,
                                    if matches!(f.ty, Type::Opt(_)) { format!(" (or `set y.{} = none`)", f.name) } else { String::new() }
                                ),
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
