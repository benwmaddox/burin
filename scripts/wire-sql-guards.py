# One-off: screen guards in SQL (params' fields as subselects, first(query) as an id).
p = 'D:/code/etch/crates/etchc/src/sql.rs'
s = open(p, encoding='utf-8').read()

def rep(old, new):
    global s
    assert s.count(old) == 1, (old[:80], s.count(old))
    s = s.replace(old, new)

rep('''struct Tx<'a> {
    m: &'a Model<'a>,
    joins: Joins,
    binds: Vec<Bind>,''', '''struct Tx<'a> {
    m: &'a Model<'a>,
    joins: Joins,
    binds: Vec<Bind>,
    /// parameters that are rows (name, entity): their fields are subselects by their id
    params: Vec<(String, String)>,''')
s = s.replace("Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], rows:", "Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: vec![], rows:")
rep('''            Expr::Name(..) | Expr::Field(..) => {
                // a value from outside the query: bound''', '''            Expr::Field(..) if self.param_path(e).is_some() => {
                // a row parameter's field: a subselect by the parameter's id (bound)
                let (root, ent, path) = self.param_path(e).unwrap();
                self.binds.push(Bind::Path(Expr::Name(root, e.span())));
                self.sub += 1;
                let a = format!("p{}", self.sub);
                let col = sub_column(self.m, &a, &ent, &path)?;
                format!("(SELECT {col} FROM {} {a} WHERE {a}.id = ?)", q(&ent))
            }
            Expr::Call(f, args, _) if f == "first" => {
                let qy = match &args[0] {
                    Expr::Query(x) => (**x).clone(),
                    _ => return Err("first takes a query".into()),
                };
                let (from, w) = self.subquery(&args[0])?;
                let (alias, _) = self.rows.last().cloned().unwrap();
                let mut order = vec![];
                for (k, d) in &qy.order {
                    order.push(format!("{}{}", self.expr(k)?, if *d { " DESC" } else { "" }));
                }
                self.rows.pop();
                let ord = if order.is_empty() { String::new() } else { format!(" ORDER BY {}", order.join(", ")) };
                format!("(SELECT {alias}.id {from}{w}{ord} LIMIT 1)")
            }
            Expr::Name(..) | Expr::Field(..) => {
                // a value from outside the query: bound''')
rep('''impl<'a> Tx<'a> {
    fn expr(''', '''impl<'a> Tx<'a> {
    /// A path from a row parameter (`room.type`): its root, its entity, the fields.
    fn param_path(&self, e: &Expr) -> Option<(String, String, Vec<String>)> {
        fn walk(e: &Expr) -> Option<(String, Vec<String>)> {
            match e {
                Expr::Name(n, _) => Some((n.clone(), vec![])),
                Expr::Field(o, f, _) => {
                    let (r, mut p) = walk(o)?;
                    p.push(f.clone());
                    Some((r, p))
                }
                _ => None,
            }
        }
        let (r, p) = walk(e)?;
        if p.is_empty() || p == ["id"] {
            return None;
        }
        let ent = self.params.iter().find(|(n, _)| *n == r)?.1.clone();
        Some((r, ent, p))
    }

    fn expr(''')
rep('''pub fn cond_select(m: &Model, cond: &Expr) -> Result<Select, String> {
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: vec![], rows: vec![], sub: 0 };''', '''pub fn cond_select(m: &Model, cond: &Expr) -> Result<Select, String> {
    cond_select_with(m, cond, &[])
}

/// A condition as one SQL expression, with row parameters (name, entity) whose fields it reads.
pub fn cond_select_with(m: &Model, cond: &Expr, params: &[(String, String)]) -> Result<Select, String> {
    let mut tx = Tx { m, joins: Joins { list: vec![], sql: String::new(), next: 0 }, binds: vec![], params: params.to_vec(), rows: vec![], sub: 0 };''')
open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('ok')
