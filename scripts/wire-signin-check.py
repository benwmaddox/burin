root = '/mnt/d/code/etch/crates/etchc/src/'

def edit(path, pairs):
    p = root + path
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, (path, old[:70])
        s = s.replace(old, new, 1)
    open(p, 'w', encoding='utf-8').write(s)

edit('check.rs', [
    # the model keeps the home declaration
    ('''    pub screens: Vec<&'a Decl>,
    pub actions: Vec<&'a Decl>,
}''', '''    pub screens: Vec<&'a Decl>,
    pub actions: Vec<&'a Decl>,
    pub home: Option<&'a Decl>,
}'''),
    ('''            screens: vec![],
            actions: vec![],
        };''', '''            screens: vec![],
            actions: vec![],
            home: None,
        };'''),
    ('''                Decl::Action { .. } => m.actions.push(d),
            }''', '''                Decl::Action { .. } => m.actions.push(d),
                Decl::Home { span, .. } => {
                    if m.home.replace(d).is_some() {
                        err(*span, "there is one home declaration")?
                    }
                }
            }'''),
    # calls
    ('''            "first" => {
                arity(1)?;
                rows(0)
            }''', '''            "first" => {
                arity(1)?;
                rows(0)
            }
            // a password's hash checked against text (in an action's require only: the runtime
            // verifies it off the writer)
            "authenticates" => {
                arity(2)?;
                if !matches!(self.type_of(&args[0], scope)?.strip_opt(), Type::Password) {
                    return err(s, "authenticates takes a Password field, then the text to check");
                }
                arg(1, Type::Text)?;
                Ok(Type::Bool)
            }
            // the request's cookie, as an id (none when absent or not a number)
            "cookie" => {
                arity(1)?;
                match &args[0] {
                    Expr::Str(..) => Ok(Type::Opt(Box::new(Type::Int))),
                    _ => err(s, "cookie takes a literal name"),
                }
            }
            // the first value that isn't none
            "either" => {
                arity(2)?;
                let a = self.type_of(&args[0], scope)?;
                let b = self.type_of(&args[1], scope)?;
                if a.strip_opt() != b.strip_opt() {
                    return err(s, format!("either takes two of the same type, not {} and {}", a.show(), b.show()));
                }
                Ok(b)
            }'''),
    # screens: public ones take no parameters and no guards, and don't read the viewer
    ('''            Decl::Screen { params, requires, cookies, body, route, span, .. } => {
                let scope = Scope::of(params);''', '''            Decl::Screen { params, requires, cookies, body, route, span, public, .. } => {
                if *public && (!params.is_empty() || !requires.is_empty() || !cookies.is_empty() || nodes_mention_viewer(m, &body.nodes)) {
                    return err(*span, "a public screen has no parameters, guards or cookies, and doesn't read the viewer");
                }
                let scope = Scope::of(params);'''),
    ('''            Decl::Action { params, body, .. } => check_stmts(m, body, &Scope::of(params), &mut vec![])?,''',
     '''            Decl::Action { params, body, public, span, .. } => {
                if *public && body.iter().any(stmt_mentions_viewer) {
                    return err(*span, "a public action runs without a viewer: it can't read `viewer`");
                }
                check_stmts(m, body, &Scope::of(params), &mut vec![])?
            }
            Decl::Home { screen, arg, span } => {
                let want = m.screens.iter().find_map(|d| match d {
                    Decl::Screen { name, params, .. } if name == screen && params.len() == 1 => Some(params[0].ty.clone()),
                    _ => None,
                });
                let Some(want) = want else { return err(*span, format!("home names a screen with one row parameter, not `{screen}`")) };
                let got = m.type_of(arg, &Scope::default())?;
                if got.strip_opt() != want.strip_opt() {
                    return err(arg.span(), format!("home's {screen} takes {}, not {}", want.show(), got.show()));
                }
            }'''),
    # statements
    ('''fn check_stmts(m: &Model, body: &[Stmt], scope: &Scope, cleared: &mut Vec<(String, String, String)>) -> R<()> {
    for s in body {''', '''fn check_stmts(m: &Model, body: &[Stmt], scope: &Scope, cleared: &mut Vec<(String, String, String)>) -> R<()> {
    let mut scope = scope.clone();
    let scope = &mut scope;
    for s in body {'''),
    ('''            Stmt::Delete(e, span) => {''', '''            Stmt::Let(n, e, span) => {
                if scope.get(n).is_some() {
                    return err(*span, format!("`{n}` is already a name here"));
                }
                let t = m.type_of(e, scope)?;
                *scope = scope.with(n, t);
            }
            Stmt::SignIn(e, span) => {
                let Some(Decl::Viewer { entity, via_entity, via_field, to_field, .. }) = m.viewer else { return err(*span, "sign in needs a viewer declaration") };
                if m.type_of(e, scope)?.strip_opt() != &Type::Ref(entity.clone()) {
                    return err(*span, format!("sign in takes a {entity}"));
                }
                // the session row is made from the user and a new token: nothing else it needs
                if let Some(f) = m.entities[via_entity].fields.iter().find(|f| f.name != *via_field && f.name != *to_field && !matches!(f.ty, Type::Opt(_))) {
                    return err(*span, format!("sign in makes a {via_entity} from its {to_field} and {via_field} alone; {via_entity}.{} needs a value", f.name));
                }
            }
            Stmt::SignOut(span) => {
                if m.viewer.is_none() {
                    return err(*span, "sign out needs a viewer declaration");
                }
            }
            Stmt::Delete(e, span) => {'''),
])

# the helpers
p = root + 'check.rs'
s = open(p, encoding='utf-8').read()
s += '''
fn expr_mentions_viewer(e: &Expr) -> bool {
    match e {
        Expr::Name(n, _) => n == "viewer",
        Expr::Field(o, _, _) | Expr::Not(o, _) => expr_mentions_viewer(o),
        Expr::Bin(_, l, r, _) => expr_mentions_viewer(l) || expr_mentions_viewer(r),
        Expr::Call(_, a, _) | Expr::List(a, _) => a.iter().any(expr_mentions_viewer),
        Expr::If(c, a, b, _) => expr_mentions_viewer(c) || expr_mentions_viewer(a) || expr_mentions_viewer(b),
        Expr::Record(fs, _) => fs.iter().any(|(_, v)| expr_mentions_viewer(v)),
        Expr::Comp(_, x, q, _) => expr_mentions_viewer(x) || expr_mentions_viewer(&Expr::Query(q.clone())),
        Expr::Query(q) => {
            q.filter.as_ref().map(expr_mentions_viewer).unwrap_or(false)
                || matches!(&q.source, Source::Expr(x) if expr_mentions_viewer(x))
                || q.order.iter().any(|(k, _)| expr_mentions_viewer(k))
        }
        _ => false,
    }
}

fn stmt_mentions_viewer(s: &Stmt) -> bool {
    match s {
        Stmt::Require(r) => expr_mentions_viewer(&r.cond),
        Stmt::Create(_, fs, _) => fs.iter().any(|(_, v)| expr_mentions_viewer(v)),
        Stmt::Touch(e, _) | Stmt::Delete(e, _) | Stmt::Let(_, e, _) | Stmt::SignIn(e, _) => expr_mentions_viewer(e),
        Stmt::Set(o, _, v, _) => expr_mentions_viewer(o) || expr_mentions_viewer(v),
        Stmt::ForEach(_, q, inner, _) => expr_mentions_viewer(&Expr::Query(Box::new(q.clone()))) || inner.iter().any(stmt_mentions_viewer),
        Stmt::SignOut(_) => true,
    }
}

/// Whether a template reads the viewer (through the functions and fragments it calls, too).
fn nodes_mention_viewer(m: &Model, nodes: &[TNode]) -> bool {
    let inlined = crate::inline::nodes(m, nodes, &HashMap::new());
    fn walk(m: &Model, ns: &[TNode]) -> bool {
        ns.iter().any(|n| match n {
            TNode::Text(_) => false,
            TNode::Hole(e) => {
                expr_mentions_viewer(e)
                    || matches!(e, Expr::Call(f, _, _) if matches!(m.fragments.get(f), Some(Decl::Fragment { body, per_viewer, .. }) if *per_viewer || walk(m, &crate::inline::nodes(m, &body.nodes, &HashMap::new()))))
            }
            TNode::If(c, a, b) => expr_mentions_viewer(c) || walk(m, a) || walk(m, b),
            TNode::For(_, l, _, b) => expr_mentions_viewer(l) || walk(m, b),
        })
    }
    walk(m, &inlined)
}
'''
open(p, 'w', encoding='utf-8').write(s)

# netlist: the new statements write what they make
edit('netlist.rs', [
    ('''            Stmt::Require(_) => {}
        }''', '''            Stmt::SignIn(..) | Stmt::SignOut(_) => {
                if let Some(Decl::Viewer { via_entity, .. }) = m.viewer {
                    out.push((via_entity.clone(), vec!["*".into()]));
                }
            }
            Stmt::Require(_) | Stmt::Let(..) => {}
        }'''),
])
print('ok')
